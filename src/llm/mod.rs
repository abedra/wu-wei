mod anthropic;
mod openai;
mod prompt;

use std::sync::mpsc::{self, Receiver};
use std::thread;

use chrono::NaiveDate;

use crate::domain::task::{Recurrence, TaskId};

/// A task as extracted from free text by an LLM, before it's turned into a
/// real [`crate::domain::task::Task`] (which needs a project id, not a name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTask {
    pub title: String,
    pub due_date: Option<NaiveDate>,
    pub project: Option<String>,
    pub recurrence: Option<Recurrence>,
}

/// Context handed to the model so it can resolve relative dates and match an
/// existing project by name instead of inventing a new one.
pub struct PromptContext {
    pub today: NaiveDate,
    pub project_names: Vec<String>,
}

/// One turn in the AI chat panel's conversation history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub role: ChatRole,
    pub content: String,
}

/// A task summary handed to the model as the working set it can act on —
/// deliberately just enough to reference and reason about a task, not a full
/// `Task` (no notes/timestamps/estimated_minutes).
pub struct ChatTaskSummary {
    pub id: TaskId,
    pub title: String,
    pub project: Option<String>,
    pub due_date: Option<NaiveDate>,
}

pub struct ChatContext {
    pub today: NaiveDate,
    pub tasks: Vec<ChatTaskSummary>,
    pub project_names: Vec<String>,
}

/// A single database mutation the model asked for, already validated (task
/// id parses, action name recognized). DeleteTask/DeleteProject
/// are both genuinely irreversible from the UI (no undo) — the "Actions
/// taken"/"This didn't actually happen" reporting in
/// `AppState::apply_chat_reply` is what keeps them honest, not restraint on
/// which actions exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatAction {
    SetDueDate {
        task_id: TaskId,
        due_date: NaiveDate,
    },
    ClearDueDate {
        task_id: TaskId,
    },
    SetRecurrence {
        task_id: TaskId,
        recurrence: Recurrence,
    },
    ClearRecurrence {
        task_id: TaskId,
    },
    CompleteTask {
        task_id: TaskId,
    },
    ReopenTask {
        task_id: TaskId,
    },
    MoveToProject {
        task_id: TaskId,
        project: String,
    },
    MoveToInbox {
        task_id: TaskId,
    },
    CreateTask {
        task: ParsedTask,
    },
    CreateProject {
        name: String,
    },
    DeleteProject {
        project: String,
    },
    DeleteTask {
        task_id: TaskId,
    },
}

#[derive(Debug, Clone)]
pub struct ChatReply {
    pub reply: String,
    pub actions: Vec<ChatAction>,
    /// Actions the model returned that couldn't be understood (unknown
    /// type, missing a required field, unparsable id/date, ...). Reported
    /// back to the user alongside `reply` rather than discarding the whole
    /// turn — one malformed action shouldn't cost the reply text or the
    /// other, valid actions in the same response.
    pub parse_failures: Vec<String>,
}

/// Implemented once per LLM backend. Blocking by design: callers only ever
/// run it on a background thread (see [`parse_capture_async`]/
/// [`send_chat_async`]), keeping the egui loop and the synchronous `db`
/// layer untouched.
trait Provider: Send {
    fn parse(&self, raw_text: &str, context: &PromptContext) -> Result<ParsedTask, String>;
    fn chat(&self, history: &[ChatTurn], context: &ChatContext) -> Result<ChatReply, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
}

#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub provider: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl LlmConfig {
    /// Reads provider selection and credentials from the environment.
    /// Returns `None` when no API key is configured — AI-assisted capture is
    /// opt-in and simply unavailable (not an error) until the user sets it up.
    ///
    /// - `WU_WEI_LLM_PROVIDER` — `openai` (default) or `anthropic`.
    /// - `WU_WEI_LLM_API_KEY` — overrides the provider-specific env var below.
    /// - `WU_WEI_LLM_BASE_URL` — overrides the provider default (e.g. to point
    ///   the OpenAI-compatible client at a local server).
    /// - `WU_WEI_LLM_MODEL` — overrides the provider default model.
    pub fn from_env() -> Option<Self> {
        let provider = match std::env::var("WU_WEI_LLM_PROVIDER").as_deref() {
            Ok("anthropic") => ProviderKind::Anthropic,
            _ => ProviderKind::OpenAi,
        };
        let (default_key_var, default_base_url, default_model) = match provider {
            ProviderKind::OpenAi => ("OPENAI_API_KEY", "https://api.openai.com/v1", "gpt-4o-mini"),
            ProviderKind::Anthropic => (
                "ANTHROPIC_API_KEY",
                "https://api.anthropic.com",
                "claude-opus-5",
            ),
        };
        let api_key = std::env::var("WU_WEI_LLM_API_KEY")
            .or_else(|_| std::env::var(default_key_var))
            .ok()?;
        let base_url = std::env::var("WU_WEI_LLM_BASE_URL")
            .unwrap_or_else(|_| default_base_url.to_string());
        let model =
            std::env::var("WU_WEI_LLM_MODEL").unwrap_or_else(|_| default_model.to_string());
        Some(LlmConfig {
            provider,
            api_key,
            base_url,
            model,
        })
    }

    fn build_provider(&self) -> Box<dyn Provider> {
        match self.provider {
            ProviderKind::OpenAi => Box::new(openai::OpenAiProvider::new(self.clone())),
            ProviderKind::Anthropic => Box::new(anthropic::AnthropicProvider::new(self.clone())),
        }
    }
}

/// Spawns a background thread that sends `raw_text` to the configured LLM
/// and parses the reply into task fields, delivering the result over the
/// returned channel. The caller polls it once per frame with `try_recv`
/// rather than blocking the UI thread.
pub fn parse_capture_async(
    config: LlmConfig,
    raw_text: String,
    context: PromptContext,
) -> Receiver<Result<ParsedTask, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let provider = config.build_provider();
        let result = provider.parse(&raw_text, &context);
        let _ = tx.send(result);
    });
    rx
}

/// Spawns a background thread that sends the chat panel's conversation
/// history (ending with the newest user turn) to the configured LLM, and
/// delivers its reply plus any requested task actions over the returned
/// channel. Polled once per frame, same as [`parse_capture_async`].
pub fn send_chat_async(
    config: LlmConfig,
    history: Vec<ChatTurn>,
    context: ChatContext,
) -> Receiver<Result<ChatReply, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let provider = config.build_provider();
        let result = provider.chat(&history, &context);
        let _ = tx.send(result);
    });
    rx
}
