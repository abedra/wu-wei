mod anthropic;
mod openai;
mod prompt;

use std::sync::mpsc::{self, Receiver};
use std::thread;

use chrono::NaiveDate;

/// A task as extracted from free text by an LLM, before it's turned into a
/// real [`crate::domain::task::Task`] (which needs project/tag IDs, not names).
#[derive(Debug, Clone)]
pub struct ParsedTask {
    pub title: String,
    pub due_date: Option<NaiveDate>,
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub flagged: bool,
}

/// Context handed to the model so it can resolve relative dates and match an
/// existing project by name instead of inventing a new one.
pub struct PromptContext {
    pub today: NaiveDate,
    pub project_names: Vec<String>,
}

/// Implemented once per LLM backend. Blocking by design: callers only ever
/// run it on a background thread (see [`parse_capture_async`]), keeping the
/// egui loop and the synchronous `db` layer untouched.
trait Provider: Send {
    fn parse(&self, raw_text: &str, context: &PromptContext) -> Result<ParsedTask, String>;
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
    /// - `LOA_LLM_PROVIDER` — `openai` (default) or `anthropic`.
    /// - `LOA_LLM_API_KEY` — overrides the provider-specific env var below.
    /// - `LOA_LLM_BASE_URL` — overrides the provider default (e.g. to point
    ///   the OpenAI-compatible client at a local server).
    /// - `LOA_LLM_MODEL` — overrides the provider default model.
    pub fn from_env() -> Option<Self> {
        let provider = match std::env::var("LOA_LLM_PROVIDER").as_deref() {
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
        let api_key = std::env::var("LOA_LLM_API_KEY")
            .or_else(|_| std::env::var(default_key_var))
            .ok()?;
        let base_url =
            std::env::var("LOA_LLM_BASE_URL").unwrap_or_else(|_| default_base_url.to_string());
        let model = std::env::var("LOA_LLM_MODEL").unwrap_or_else(|_| default_model.to_string());
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
