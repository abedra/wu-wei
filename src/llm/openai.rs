use serde::Deserialize;
use serde_json::json;

use super::prompt::{RawExtraction, response_schema, system_prompt};
use super::{LlmConfig, ParsedTask, PromptContext, Provider};

/// OpenAI-compatible Chat Completions client. Works against the real OpenAI
/// API or any server implementing the same wire format (base URL and model
/// are both configurable via [`LlmConfig`]).
pub struct OpenAiProvider {
    config: LlmConfig,
}

impl OpenAiProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

#[derive(Deserialize)]
struct Message {
    content: String,
}

impl Provider for OpenAiProvider {
    fn parse(&self, raw_text: &str, context: &PromptContext) -> Result<ParsedTask, String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let body = json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": system_prompt(context) },
                { "role": "user", "content": raw_text },
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "task_extraction",
                    "strict": true,
                    "schema": response_schema(),
                },
            },
        });

        let mut response = ureq::post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .send_json(&body)
            .map_err(|e| format!("OpenAI-compatible request failed: {e}"))?;

        let parsed: ChatResponse = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("failed to read OpenAI-compatible response: {e}"))?;

        let content = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| "OpenAI-compatible response had no choices".to_string())?
            .message
            .content;

        let extraction: RawExtraction = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse extracted task JSON: {e}"))?;
        extraction.into_parsed_task()
    }
}
