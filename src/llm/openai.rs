use serde::Deserialize;
use serde_json::json;

use chrono::NaiveDate;

use super::prompt::{
    RawChatReply, RawDueDate, RawExtraction, chat_response_schema, chat_system_prompt,
    due_date_prompt, due_date_schema, response_schema, system_prompt,
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

    /// Adds `max_tokens` to a request body only when the user has set an
    /// explicit ceiling (`llm_max_tokens`). Left unset, the server applies its
    /// own default — the response length "inherits" from the service rather
    /// than being forced here.
    fn apply_max_tokens(&self, body: &mut serde_json::Value) {
        if let Some(max) = self.config.max_tokens {
            body["max_tokens"] = max.into();
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
    #[serde(default)]
    finish_reason: Option<String>,
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
        let mut body = json!({
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
        self.apply_max_tokens(&mut body);

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

        let mut body = json!({
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
        self.apply_max_tokens(&mut body);

        let mut response = ureq::post(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .send_json(&body)
            .map_err(|e| format!("OpenAI-compatible request failed: {e}"))?;

        let parsed: ChatResponse = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("failed to read OpenAI-compatible response: {e}"))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| "OpenAI-compatible response had no choices".to_string())?;

        if choice.finish_reason.as_deref() == Some("length") {
            return Err(
                "the response was cut off before it finished (too many changes in one \
                 request) — try asking for fewer at a time"
                    .to_string(),
            );
        }

        let content = choice.message.content;

        let raw: RawChatReply = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse chat reply JSON: {e}"))?;
        Ok(raw.into_chat_reply(&context.project_names))
    }

    fn parse_due_date(&self, text: &str, today: NaiveDate) -> Result<NaiveDate, String> {
        let url = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut body = json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": due_date_prompt(today) },
                { "role": "user", "content": text },
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "due_date",
                    "strict": true,
                    "schema": due_date_schema(),
                },
            },
        });
        self.apply_max_tokens(&mut body);

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

        let raw: RawDueDate = serde_json::from_str(&content)
            .map_err(|e| format!("failed to parse date JSON: {e}"))?;
        raw.into_date()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ProviderKind;

    fn provider(max_tokens: Option<u32>) -> OpenAiProvider {
        OpenAiProvider::new(LlmConfig {
            provider: ProviderKind::OpenAi,
            api_key: "sk-test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o-mini".to_string(),
            max_tokens,
        })
    }

    #[test]
    fn max_tokens_is_omitted_when_unset_so_the_service_decides() {
        let mut body = json!({ "model": "gpt-4o-mini" });
        provider(None).apply_max_tokens(&mut body);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn max_tokens_is_sent_when_the_user_set_a_ceiling() {
        let mut body = json!({ "model": "gpt-4o-mini" });
        provider(Some(4096)).apply_max_tokens(&mut body);
        assert_eq!(body["max_tokens"], json!(4096));
    }
}
