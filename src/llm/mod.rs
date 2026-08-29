mod anthropic;
mod openai;
mod prompt;

use std::collections::HashMap;
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

/// A task the user finished recently, handed to the model so it can write a
/// weekly review ("what did I get done this week?"). Recurring-task
/// completions are deliberately left out upstream — routine upkeep, not the
/// week's notable work. `id` is present so a follow-up like "actually reopen
/// that one" still works.
pub struct ChatCompletedTaskSummary {
    pub id: TaskId,
    pub title: String,
    pub project: Option<String>,
    pub completed_on: NaiveDate,
}

/// Today's Google Calendar events, for the model's awareness only — there's
/// no action to create/move/delete a calendar event, so this is handed over
/// purely as read-only context (e.g. "am I free this afternoon?", "what's on
/// my plate today?"). `time` is already formatted in the user's local time
/// (or "All day"), matching what `ui::task_list` shows.
pub struct ChatCalendarEventSummary {
    pub title: String,
    pub time: String,
    pub location: Option<String>,
}

pub struct ChatContext {
    pub today: NaiveDate,
    pub tasks: Vec<ChatTaskSummary>,
    pub project_names: Vec<String>,
    pub calendar_events: Vec<ChatCalendarEventSummary>,
    /// The earliest completion date covered by `completed_recently` — the
    /// start of the "week" a summary request should describe.
    pub completed_since: NaiveDate,
    /// Non-recurring tasks completed on or after `completed_since`, newest
    /// first — the material for a weekly summary.
    pub completed_recently: Vec<ChatCompletedTaskSummary>,
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
    /// Move every currently-overdue task to `due_date` in one shot. The app
    /// resolves the set itself (from `task_repo::list_overdue`) rather than
    /// trusting the model to enumerate and transcribe one id per task — a
    /// "roll all my overdue tasks to today" command otherwise routinely comes
    /// back with only some of them.
    RescheduleOverdue {
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
    /// Resolves a short free-text phrase ("next friday", "in 3 weeks") to a
    /// single calendar date — the due-date picker's typed-input field.
    fn parse_due_date(&self, text: &str, today: NaiveDate) -> Result<NaiveDate, String>;
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
    /// Resolves provider selection and credentials, preferring a value saved
    /// in `settings` (the Settings screen — see `ui::settings`/
    /// `AppState::save_settings`) over the matching environment variable,
    /// over the provider's own default. Returns `None` when no API key is
    /// available from either source: AI-assisted capture is opt-in and
    /// simply unavailable (not an error) until set up.
    ///
    /// `settings` keys: `llm_provider` (`openai`/`anthropic`), `llm_api_key`,
    /// `llm_base_url`, `llm_model` — mirroring `WU_WEI_LLM_PROVIDER`,
    /// `WU_WEI_LLM_API_KEY`, `WU_WEI_LLM_BASE_URL`, `WU_WEI_LLM_MODEL`.
    pub fn resolve(settings: &HashMap<String, String>) -> Option<Self> {
        let setting_or_env = |setting_key: &str, env_key: &str| -> Option<String> {
            settings
                .get(setting_key)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| std::env::var(env_key).ok())
        };

        let provider = match setting_or_env("llm_provider", "WU_WEI_LLM_PROVIDER").as_deref() {
            Some("anthropic") => ProviderKind::Anthropic,
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
        let api_key = setting_or_env("llm_api_key", "WU_WEI_LLM_API_KEY")
            .or_else(|| std::env::var(default_key_var).ok())?;
        let base_url = setting_or_env("llm_base_url", "WU_WEI_LLM_BASE_URL")
            .unwrap_or_else(|| default_base_url.to_string());
        let model = setting_or_env("llm_model", "WU_WEI_LLM_MODEL")
            .unwrap_or_else(|| default_model.to_string());
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

/// Spawns a background thread that resolves a free-text date phrase (from
/// the due-date picker's input field) to a single [`NaiveDate`], delivering
/// the result over the returned channel. Polled once per frame, same as
/// [`parse_capture_async`].
pub fn parse_due_date_async(
    config: LlmConfig,
    text: String,
    today: NaiveDate,
) -> Receiver<Result<NaiveDate, String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let provider = config.build_provider();
        let _ = tx.send(provider.parse_due_date(&text, today));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_saved_settings_over_provider_defaults() {
        let settings = HashMap::from([
            ("llm_provider".to_string(), "anthropic".to_string()),
            ("llm_api_key".to_string(), "sk-test".to_string()),
            (
                "llm_base_url".to_string(),
                "https://example.test".to_string(),
            ),
            ("llm_model".to_string(), "claude-test".to_string()),
        ]);

        let config = LlmConfig::resolve(&settings).unwrap();
        assert_eq!(config.provider, ProviderKind::Anthropic);
        assert_eq!(config.api_key, "sk-test");
        assert_eq!(config.base_url, "https://example.test");
        assert_eq!(config.model, "claude-test");
    }

    #[test]
    fn resolve_falls_back_to_the_provider_default_when_only_the_key_is_saved() {
        let settings = HashMap::from([
            ("llm_provider".to_string(), "anthropic".to_string()),
            ("llm_api_key".to_string(), "sk-test".to_string()),
        ]);

        let config = LlmConfig::resolve(&settings).unwrap();
        assert_eq!(config.base_url, "https://api.anthropic.com");
        assert_eq!(config.model, "claude-opus-5");
    }
}
