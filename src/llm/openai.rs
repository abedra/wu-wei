use serde::Deserialize;
use serde_json::json;

use super::prompt::{
    RawChatReply, RawExtraction, chat_response_schema, chat_system_prompt, response_schema,
    system_prompt,
};
use super::{
    ChatContext, ChatReply, ChatRole, ChatTurn, LlmConfig, ParsedTask, PromptContext, Provider,
};

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

    fn chat(&self, history: &[ChatTurn], context: &ChatContext) -> Result<ChatReply, String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut messages =
            vec![json!({ "role": "system", "content": chat_system_prompt(context) })];
        messages.extend(history.iter().map(|turn| {
            let role = match turn.role {
                ChatRole::User => "user",
                ChatRole::Assistant => "assistant",
            };
            json!({ "role": role, "content": turn.content })
        }));

        let body = json!({
            "model": self.config.model,
            "messages": messages,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "chat_actions",
                    "strict": true,
                    "schema": chat_response_schema(),
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

        let raw: RawChatReply = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse chat reply JSON: {e}"))?;
        Ok(raw.into_chat_reply(&context.project_names, &context.tag_names))
    }
}
