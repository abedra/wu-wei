use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ParsedTask, PromptContext};
use crate::domain::task::{Recurrence, RecurrenceUnit};

/// Shared instructions given to whichever provider is configured — kept
/// provider-agnostic so both HTTP clients ask the model for exactly the same
/// thing, just wrapped in different request shapes.
pub fn system_prompt(context: &PromptContext) -> String {
    let projects = if context.project_names.is_empty() {
        "(none yet)".to_string()
    } else {
        context.project_names.join(", ")
    };
    format!(
        "You turn a single line of quick-capture text into a structured task for a \
         GTD-style task manager. Today's date is {today}. Existing projects: {projects}. \
         Extract: a short imperative title with any date/project/tag/recurrence phrases \
         removed; a due_date (YYYY-MM-DD) if the text implies one, resolving relative \
         dates like \"tomorrow\" or \"next week\" against today, else null; tags as short \
         lowercase single words (from hashtags or clearly implied topics), else an empty \
         list; a project name copied exactly from the existing projects list if one is \
         clearly implied, else null — never invent a new project name; flagged as true \
         only if the text signals urgency or importance (e.g. \"urgent\", \"asap\", \
         \"important\"), else false; recurrence as {{interval, unit}} if the text implies \
         the task repeats — \"daily\"/\"every day\" -> {{interval:1, unit:\"days\"}}, \
         \"weekly\" -> {{interval:1, unit:\"weeks\"}}, \"every 3 days\" -> {{interval:3, \
         unit:\"days\"}}, \"monthly\" -> {{interval:1, unit:\"months\"}} — else null.",
        today = context.today.format("%Y-%m-%d"),
        projects = projects,
    )
}

/// JSON Schema shared by both providers' structured-output request fields.
/// Nullable fields use `anyOf` rather than a `"type": [...]` array since
/// that's the form both OpenAI's and Anthropic's structured outputs document
/// as supported.
pub fn response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "due_date": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "tags": { "type": "array", "items": { "type": "string" } },
            "project": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "flagged": { "type": "boolean" },
            "recurrence": {
                "anyOf": [
                    {
                        "type": "object",
                        "properties": {
                            "interval": { "type": "integer" },
                            "unit": { "type": "string", "enum": ["days", "weeks", "months"] }
                        },
                        "required": ["interval", "unit"],
                        "additionalProperties": false
                    },
                    { "type": "null" }
                ]
            }
        },
        "required": ["title", "due_date", "tags", "project", "flagged", "recurrence"],
        "additionalProperties": false
    })
}

#[derive(Deserialize)]
pub struct RawRecurrence {
    pub interval: u32,
    pub unit: String,
}

/// The raw shape a provider's JSON reply is deserialized into, before
/// validation/parsing turns it into a [`ParsedTask`].
#[derive(Deserialize)]
pub struct RawExtraction {
    pub title: String,
    pub due_date: Option<String>,
    pub tags: Vec<String>,
    pub project: Option<String>,
    pub flagged: bool,
    pub recurrence: Option<RawRecurrence>,
}

impl RawExtraction {
    pub fn into_parsed_task(self) -> Result<ParsedTask, String> {
        let title = self.title.trim().to_string();
        if title.is_empty() {
            return Err("model returned an empty title".to_string());
        }
        let due_date = match self.due_date {
            Some(s) if !s.trim().is_empty() => Some(
                NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
                    .map_err(|e| format!("model returned an unparsable due_date {s:?}: {e}"))?,
            ),
            _ => None,
        };
        let recurrence = match self.recurrence {
            Some(r) => {
                if r.interval == 0 {
                    return Err("model returned a recurrence interval of 0".to_string());
                }
                let unit = RecurrenceUnit::parse(&r.unit).ok_or_else(|| {
                    format!("model returned an unknown recurrence unit {:?}", r.unit)
                })?;
                Some(Recurrence {
                    interval: r.interval,
                    unit,
                })
            }
            None => None,
        };
        Ok(ParsedTask {
            title,
            due_date,
            tags: self.tags,
            project: self.project.filter(|p| !p.trim().is_empty()),
            flagged: self.flagged,
            recurrence,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extraction(title: &str, due_date: Option<&str>, project: Option<&str>) -> RawExtraction {
        RawExtraction {
            title: title.to_string(),
            due_date: due_date.map(str::to_string),
            tags: vec!["errand".to_string()],
            project: project.map(str::to_string),
            flagged: true,
            recurrence: None,
        }
    }

    #[test]
    fn parses_a_full_extraction() {
        let parsed = extraction("buy milk", Some("2026-01-05"), Some("Groceries"))
            .into_parsed_task()
            .unwrap();
        assert_eq!(parsed.title, "buy milk");
        assert_eq!(
            parsed.due_date,
            Some(NaiveDate::from_ymd_opt(2026, 1, 5).unwrap())
        );
        assert_eq!(parsed.project, Some("Groceries".to_string()));
        assert_eq!(parsed.tags, vec!["errand".to_string()]);
        assert!(parsed.flagged);
    }

    #[test]
    fn rejects_an_empty_title() {
        let err = extraction("   ", None, None)
            .into_parsed_task()
            .unwrap_err();
        assert!(err.contains("empty title"));
    }

    #[test]
    fn rejects_an_unparsable_due_date() {
        let err = extraction("buy milk", Some("next tuesday"), None)
            .into_parsed_task()
            .unwrap_err();
        assert!(err.contains("unparsable due_date"));
    }

    #[test]
    fn treats_blank_due_date_and_project_as_absent() {
        let parsed = extraction("buy milk", Some(""), Some("  "))
            .into_parsed_task()
            .unwrap();
        assert_eq!(parsed.due_date, None);
        assert_eq!(parsed.project, None);
    }

    #[test]
    fn parses_a_recurrence() {
        let mut raw = extraction("inbox zero", None, None);
        raw.recurrence = Some(RawRecurrence {
            interval: 1,
            unit: "days".to_string(),
        });
        let parsed = raw.into_parsed_task().unwrap();
        assert_eq!(
            parsed.recurrence,
            Some(Recurrence {
                interval: 1,
                unit: RecurrenceUnit::Days,
            })
        );
    }

    #[test]
    fn rejects_a_zero_recurrence_interval() {
        let mut raw = extraction("inbox zero", None, None);
        raw.recurrence = Some(RawRecurrence {
            interval: 0,
            unit: "days".to_string(),
        });
        let err = raw.into_parsed_task().unwrap_err();
        assert!(err.contains("interval of 0"));
    }

    #[test]
    fn rejects_an_unknown_recurrence_unit() {
        let mut raw = extraction("inbox zero", None, None);
        raw.recurrence = Some(RawRecurrence {
            interval: 1,
            unit: "fortnights".to_string(),
        });
        let err = raw.into_parsed_task().unwrap_err();
        assert!(err.contains("unknown recurrence unit"));
    }
}
