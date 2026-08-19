use serde::Deserialize;
use serde_json::json;

use super::prompt::{
    RawChatReply, RawExtraction, chat_response_schema, chat_system_prompt, response_schema,
    system_prompt,
};
use super::{
    ChatContext, ChatReply, ChatRole, ChatTurn, LlmConfig, ParsedTask, PromptContext, Provider,
};

/// No official Rust SDK exists for the Anthropic API, so this talks to the
/// Messages API directly over HTTP (`POST {base_url}/v1/messages`), mirroring
/// [`super::openai::OpenAiProvider`]'s shape so the two stay interchangeable
/// behind the `Provider` trait.
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    config: LlmConfig,
}

impl AnthropicProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self { config }
    }
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(default)]
    text: Option<String>,
}

impl Provider for AnthropicProvider {
    fn parse(&self, raw_text: &str, context: &PromptContext) -> Result<ParsedTask, String> {
        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));
        let body = json!({
            "model": self.config.model,
            "max_tokens": 1024,
            "system": system_prompt(context),
            "messages": [
                { "role": "user", "content": raw_text },
            ],
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": response_schema(),
                },
            },
        });

        let mut response = ureq::post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send_json(&body)
            .map_err(|e| format!("Anthropic request failed: {e}"))?;

        let parsed: MessagesResponse = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("failed to read Anthropic response: {e}"))?;

        let content = parsed
            .content
            .into_iter()
            .find_map(|block| block.text)
            .ok_or_else(|| "Anthropic response had no text content".to_string())?;

        let extraction: RawExtraction = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse extracted task JSON: {e}"))?;
        extraction.into_parsed_task()
    }

    fn chat(&self, history: &[ChatTurn], context: &ChatContext) -> Result<ChatReply, String> {
        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));
        let messages: Vec<_> = history
            .iter()
            .map(|turn| {
                let role = match turn.role {
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                };
                json!({ "role": role, "content": turn.content })
            })
            .collect();

        let body = json!({
            "model": self.config.model,
            // Higher than the capture-parse call: a command like "roll all
            // overdue tasks to today" can emit one action per task.
            "max_tokens": 4096,
            "system": chat_system_prompt(context),
            "messages": messages,
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": chat_response_schema(),
                },
            },
        });

        let mut response = ureq::post(&url)
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send_json(&body)
            .map_err(|e| format!("Anthropic request failed: {e}"))?;

        let parsed: MessagesResponse = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("failed to read Anthropic response: {e}"))?;

        let content = parsed
            .content
            .into_iter()
            .find_map(|block| block.text)
            .ok_or_else(|| "Anthropic response had no text content".to_string())?;

        let raw: RawChatReply = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse chat reply JSON: {e}"))?;
        Ok(raw.into_chat_reply(&context.project_names, &context.tag_names))
    }
}
