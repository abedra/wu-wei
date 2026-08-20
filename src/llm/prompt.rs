use chrono::{Duration, NaiveDate};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ChatAction, ChatContext, ChatReply, ParsedTask, PromptContext};
use crate::domain::task::{Recurrence, RecurrenceUnit, TaskId};

/// A precomputed weekday→date lookup for today plus the next 6 days, handed
/// to the model alongside "today's date" so resolving something like "due
/// Saturday" is a lookup rather than two independent, error-prone
/// arithmetic steps (what weekday is today, then what date is the next
/// Saturday) — small models get this wrong (e.g. naming a date "Saturday"
/// that's actually a Friday).
fn weekday_reference(today: NaiveDate) -> String {
    (0..7)
        .map(|i| {
            let date = today + Duration::days(i);
            let weekday = date.format("%A");
            let date_str = date.format("%Y-%m-%d");
            if i == 0 {
                format!("{weekday}={date_str} (today)")
            } else {
                format!("{weekday}={date_str}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

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
         GTD-style task manager. Today's date is {today} ({today_weekday}). For resolving \
         relative or named-weekday dates, here are the next 7 days: {weekdays} — use this \
         lookup rather than computing weekdays yourself. Existing projects: {projects}. \
         Extract: a short imperative title with any date/project/recurrence phrases \
         removed; a due_date (YYYY-MM-DD) if the text implies one, resolving relative \
         dates like \"tomorrow\" or \"next week\" and named weekdays like \"Friday\" \
         against the lookup above, else null; a project name copied exactly from the \
         existing projects list if one is clearly implied, else null — never invent a \
         new project name; recurrence as \
         {{interval, unit}} if the text implies \
         the task repeats — \"daily\"/\"every day\" -> {{interval:1, unit:\"days\"}}, \
         \"weekly\" -> {{interval:1, unit:\"weeks\"}}, \"every 3 days\" -> {{interval:3, \
         unit:\"days\"}}, \"monthly\" -> {{interval:1, unit:\"months\"}} — else null.",
        today = context.today.format("%Y-%m-%d"),
        today_weekday = context.today.format("%A"),
        weekdays = weekday_reference(context.today),
        projects = projects,
    )
}

/// The `recurrence` field's JSON Schema — shared between `response_schema`
/// (quick capture) and `chat_response_schema` (create_task), since both
/// accept the same `{interval, unit}` shape.
fn recurrence_schema() -> Value {
    json!({
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
    })
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
            "project": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
            "recurrence": recurrence_schema()
        },
        "required": ["title", "due_date", "project", "recurrence"],
        "additionalProperties": false
    })
}

#[derive(Deserialize)]
pub struct RawRecurrence {
    pub interval: u32,
    pub unit: String,
}

fn parse_optional_due_date(raw: Option<String>) -> Result<Option<NaiveDate>, String> {
    match raw {
        Some(s) if !s.trim().is_empty() => NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
            .map(Some)
            .map_err(|e| format!("model returned an unparsable due_date {s:?}: {e}")),
        _ => Ok(None),
    }
}

fn parse_optional_recurrence(raw: Option<RawRecurrence>) -> Result<Option<Recurrence>, String> {
    match raw {
        Some(r) => {
            if r.interval == 0 {
                return Err("model returned a recurrence interval of 0".to_string());
            }
            let unit = RecurrenceUnit::parse(&r.unit)
                .ok_or_else(|| format!("model returned an unknown recurrence unit {:?}", r.unit))?;
            Ok(Some(Recurrence {
                interval: r.interval,
                unit,
            }))
        }
        None => Ok(None),
    }
}

fn parse_required_recurrence(kind: &str, raw: Option<RawRecurrence>) -> Result<Recurrence, String> {
    parse_optional_recurrence(raw)?
        .ok_or_else(|| format!("model's {kind:?} action is missing a recurrence"))
}

/// The raw shape a provider's JSON reply is deserialized into, before
/// validation/parsing turns it into a [`ParsedTask`].
#[derive(Deserialize)]
pub struct RawExtraction {
    pub title: String,
    pub due_date: Option<String>,
    pub project: Option<String>,
    pub recurrence: Option<RawRecurrence>,
}

impl RawExtraction {
    pub fn into_parsed_task(self) -> Result<ParsedTask, String> {
        let title = self.title.trim().to_string();
        if title.is_empty() {
            return Err("model returned an empty title".to_string());
        }
        Ok(ParsedTask {
            title,
            due_date: parse_optional_due_date(self.due_date)?,
            project: self.project.filter(|p| !p.trim().is_empty()),
            recurrence: parse_optional_recurrence(self.recurrence)?,
        })
    }
}

/// System prompt for the AI chat panel: gives the model the user's current
/// open tasks so it can resolve references like "the dentist task" or "all
/// overdue tasks" to concrete ids, and the fixed vocabulary of actions it's
/// allowed to request.
pub fn chat_system_prompt(context: &ChatContext) -> String {
    let projects = if context.project_names.is_empty() {
        "(none yet)".to_string()
    } else {
        context.project_names.join(", ")
    };
    let tasks_json = serde_json::to_string(
        &context
            .tasks
            .iter()
            .map(|t| {
                json!({
                    "id": t.id.0.to_string(),
                    "title": t.title,
                    "project": t.project,
                    "due_date": t.due_date.map(|d| d.format("%Y-%m-%d").to_string()),
                })
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());

    format!(
        "You are an assistant embedded in Wu Wei, a GTD-style task manager. The user gives you \
         commands about their tasks in natural language, e.g. \"roll all of my overdue tasks \
         to today\" or \"move the dentist task to Health\". Today's date is {today} \
         ({today_weekday}). \
         For resolving relative or named-weekday dates (e.g. \"due Saturday\", \"next \
         Friday\"), here are the next 7 days: {weekdays} — use this lookup rather than \
         computing weekdays yourself; getting the day-of-week arithmetic wrong is a common \
         mistake, so always check a date you resolve against this list rather than trusting \
         your own calculation. Existing projects: \
         {projects}. Here is the user's current set of open (not \
         completed) tasks as JSON — only ever reference a task by an \"id\" value copied \
         verbatim from this list, never invent one: {tasks_json}\n\n\
         Recurrence is a real, fully automatic feature of this app — a task with a recurrence \
         set (interval + unit: days/weeks/months) does not need you or the user to ever \
         manually create its next occurrence. When that task is completed, the app itself \
         creates the next one, due `interval` `unit` after the completion date. Never tell the \
         user recurrence isn't supported or that they need to create future instances by hand \
         — that's wrong. If the user asks for a recurring/repeating task, set its recurrence \
         (via create_task if it's new, or set_recurrence if it already exists) rather than \
         just setting one due date.\n\n\
         Respond with a short, conversational reply for the user, plus a list of actions to \
         perform. Each action has a \"type\" of one of: set_due_date (task_id, due_date as \
         YYYY-MM-DD, resolving relative dates like \"today\"/\"tomorrow\" against today), \
         clear_due_date (task_id), set_recurrence (task_id, recurrence — {{interval, unit}}; \
         makes an existing task repeat, or changes how often it already does), \
         clear_recurrence (task_id — makes an existing task stop repeating; it still keeps \
         whatever due date it has), complete_task (task_id), reopen_task (task_id), \
         move_to_project (task_id, project — copied exactly \
         from the existing projects list, never invent one), move_to_inbox (task_id), \
         create_project (project — the new project's name; if a project with that \
         name already exists, don't recreate it), delete_project (project — copied exactly \
         from the existing projects list; this does not delete that project's tasks, they move \
         to the Inbox, same as deleting a project from the app's UI), delete_task (task_id — \
         permanently deletes the task itself; only use this when the user clearly wants the \
         task gone for good, not \
         just done or filed away — complete_task or move_to_inbox are usually what they mean), \
         create_task (title, due_date, project, recurrence — same fields and \
         rules as quick capture; title is required, everything else is optional; project must \
         match an existing project exactly or be null, never invent one; this is the only way \
         to add a brand new task — never try to create one by sending other actions against a \
         task_id that doesn't exist in the list above). create_project/delete_project/\
         create_task act on a project or a not-yet-existing task, so leave \
         task_id null for them; every other action, including delete_task, needs a task_id \
         taken from the open-tasks list above. If a command implies several tasks \
         (e.g. \"all overdue tasks\"), emit one action per matching task. If the request is \
         ambiguous or no task/project clearly matches, ask for clarification in your reply \
         and return an empty actions list rather than guessing. Every field an action's type \
         needs (see above) must actually be filled in with a real value, never left null or \
         empty — an action with a missing field is dropped and does not happen. Only describe \
         something as done in your reply if you actually included a fully-filled-in action for \
         it; never describe an action you didn't include, or one you included but left a \
         required field empty.",
        today = context.today.format("%Y-%m-%d"),
        today_weekday = context.today.format("%A"),
        weekdays = weekday_reference(context.today),
        projects = projects,
        tasks_json = tasks_json,
    )
}

/// JSON Schema for the chat panel's structured reply. Like `response_schema`,
/// every field (including nested action fields) is required with nullable
/// ones expressed via `anyOf`, since both providers' strict structured
/// outputs disallow optional properties.
pub fn chat_response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "reply": { "type": "string" },
            "actions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "type": {
                            "type": "string",
                            "enum": [
                                "set_due_date", "clear_due_date", "set_recurrence",
                                "clear_recurrence", "complete_task", "reopen_task",
                                "move_to_project", "move_to_inbox",
                                "create_project", "delete_project",
                                "delete_task", "create_task"
                            ]
                        },
                        "task_id": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                        "due_date": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                        "project": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                        "title": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                        "recurrence": recurrence_schema()
                    },
                    "required": [
                        "type", "task_id", "due_date", "project", "title",
                        "recurrence"
                    ],
                    "additionalProperties": false
                }
            }
        },
        "required": ["reply", "actions"],
        "additionalProperties": false
    })
}

#[derive(Deserialize)]
pub struct RawChatAction {
    #[serde(rename = "type")]
    pub kind: String,
    pub task_id: Option<String>,
    pub due_date: Option<String>,
    pub project: Option<String>,
    pub title: Option<String>,
    pub recurrence: Option<RawRecurrence>,
}

#[derive(Deserialize)]
pub struct RawChatReply {
    pub reply: String,
    pub actions: Vec<RawChatAction>,
}

impl RawChatReply {
    /// Deliberately infallible: one action the model got wrong (missing a
    /// field, an unknown type, ...) shouldn't cost the reply text or every
    /// other, validly-formed action in the same response — see
    /// `ChatReply::parse_failures`.
    ///
    /// `project_names` is the real, current list (from `ChatContext`) — used
    /// to recover a `delete_project` action whose `project` field the model
    /// left empty, by checking whether its reply names exactly one of them.
    /// This is more reliable than quote-spotting since it's checked against
    /// ground truth rather than punctuation the model may not have used.
    pub fn into_chat_reply(self, project_names: &[String]) -> ChatReply {
        // Weaker models sometimes name a new/target project only in their
        // prose reply (e.g. "Created new project 'Wei-wu'." or "Deleted the
        // Recurring project.") and leave the matching action's `project`
        // field empty despite the prompt saying not to. When that's the
        // failure, try to recover the name from the reply rather than
        // reporting an action the model clearly did intend.
        let quoted_name = single_quoted_name(&self.reply);
        let known_project = single_known_name_mentioned(&self.reply, project_names);

        let mut actions = Vec::new();
        let mut parse_failures = Vec::new();
        for raw in self.actions {
            let kind = raw.kind.clone();
            match raw.into_chat_action() {
                Ok(action) => actions.push(action),
                Err(e) if e.contains("missing a project") => match kind.as_str() {
                    "create_project" => match &quoted_name {
                        // A newly-named project can't be checked against
                        // the existing list, so quoting is the only signal.
                        Some(name) => {
                            actions.push(ChatAction::CreateProject { name: name.clone() })
                        }
                        None => parse_failures.push(e),
                    },
                    "delete_project" => match known_project.as_ref().or(quoted_name.as_ref()) {
                        Some(project) => actions.push(ChatAction::DeleteProject {
                            project: project.clone(),
                        }),
                        None => parse_failures.push(e),
                    },
                    _ => parse_failures.push(e),
                },
                Err(e) => parse_failures.push(e),
            }
        }
        ChatReply {
            reply: self.reply,
            actions,
            parse_failures,
        }
    }
}

/// The text between a matching pair of `'` or `"` quotes in `text`, if
/// exactly one such pair exists — multiple or zero quoted spans are
/// ambiguous, so this deliberately returns `None` rather than guess.
fn single_quoted_name(text: &str) -> Option<String> {
    let mut spans = Vec::new();
    for quote in ['\'', '"'] {
        let mut rest = text;
        while let Some(start) = rest.find(quote) {
            let after_start = &rest[start + quote.len_utf8()..];
            let Some(end) = after_start.find(quote) else {
                break;
            };
            let content = after_start[..end].trim();
            if !content.is_empty() {
                spans.push(content.to_string());
            }
            rest = &after_start[end + quote.len_utf8()..];
        }
    }
    spans.dedup();
    match spans.len() {
        1 => spans.into_iter().next(),
        _ => None,
    }
}

/// Which of `known_names` (a project name list) appears (case-
/// insensitively) as a substring of `text`, if exactly one does — checked
/// against the real list rather than punctuation, so it doesn't depend on
/// the model having quoted the name at all.
fn single_known_name_mentioned(text: &str, known_names: &[String]) -> Option<String> {
    let lower = text.to_lowercase();
    let mut matches: Vec<&String> = known_names
        .iter()
        .filter(|name| !name.trim().is_empty() && lower.contains(&name.to_lowercase()))
        .collect();
    matches.dedup();
    match matches.len() {
        1 => matches.into_iter().next().cloned(),
        _ => None,
    }
}

fn parse_required_due_date(kind: &str, raw: Option<String>) -> Result<NaiveDate, String> {
    let s = raw
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("model's {kind:?} action is missing a due_date"))?;
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .map_err(|e| format!("model returned an unparsable due_date {s:?}: {e}"))
}

fn parse_required_project(kind: &str, raw: Option<String>) -> Result<String, String> {
    raw.filter(|p| !p.trim().is_empty())
        .ok_or_else(|| format!("model's {kind:?} action is missing a project"))
}

fn parse_required_task_id(kind: &str, raw: Option<String>) -> Result<TaskId, String> {
    let s = raw
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("model's {kind:?} action is missing a task_id"))?;
    let uuid = uuid::Uuid::parse_str(s.trim())
        .map_err(|e| format!("model returned an unparsable task_id {s:?}: {e}"))?;
    Ok(TaskId(uuid))
}

impl RawChatAction {
    fn into_chat_action(self) -> Result<ChatAction, String> {
        match self.kind.as_str() {
            "set_due_date" => Ok(ChatAction::SetDueDate {
                task_id: parse_required_task_id("set_due_date", self.task_id)?,
                due_date: parse_required_due_date("set_due_date", self.due_date)?,
            }),
            "clear_due_date" => Ok(ChatAction::ClearDueDate {
                task_id: parse_required_task_id("clear_due_date", self.task_id)?,
            }),
            "set_recurrence" => Ok(ChatAction::SetRecurrence {
                task_id: parse_required_task_id("set_recurrence", self.task_id)?,
                recurrence: parse_required_recurrence("set_recurrence", self.recurrence)?,
            }),
            "clear_recurrence" => Ok(ChatAction::ClearRecurrence {
                task_id: parse_required_task_id("clear_recurrence", self.task_id)?,
            }),
            "complete_task" => Ok(ChatAction::CompleteTask {
                task_id: parse_required_task_id("complete_task", self.task_id)?,
            }),
            "reopen_task" => Ok(ChatAction::ReopenTask {
                task_id: parse_required_task_id("reopen_task", self.task_id)?,
            }),
            "move_to_project" => Ok(ChatAction::MoveToProject {
                task_id: parse_required_task_id("move_to_project", self.task_id)?,
                project: parse_required_project("move_to_project", self.project)?,
            }),
            "move_to_inbox" => Ok(ChatAction::MoveToInbox {
                task_id: parse_required_task_id("move_to_inbox", self.task_id)?,
            }),
            "create_project" => Ok(ChatAction::CreateProject {
                name: parse_required_project("create_project", self.project)?,
            }),
            "delete_project" => Ok(ChatAction::DeleteProject {
                project: parse_required_project("delete_project", self.project)?,
            }),
            "delete_task" => Ok(ChatAction::DeleteTask {
                task_id: parse_required_task_id("delete_task", self.task_id)?,
            }),
            "create_task" => {
                let title = self
                    .title
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .ok_or_else(|| {
                        "model's \"create_task\" action is missing a title".to_string()
                    })?;
                Ok(ChatAction::CreateTask {
                    task: ParsedTask {
                        title,
                        due_date: parse_optional_due_date(self.due_date)?,
                        project: self.project.filter(|p| !p.trim().is_empty()),
                        recurrence: parse_optional_recurrence(self.recurrence)?,
                    },
                })
            }
            other => Err(format!("model returned an unknown action type {other:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_reference_covers_today_through_six_days_out_with_correct_names() {
        // 2026-08-19 is a Wednesday.
        let today = NaiveDate::from_ymd_opt(2026, 8, 19).unwrap();
        let reference = weekday_reference(today);
        assert_eq!(
            reference,
            "Wednesday=2026-08-19 (today), Thursday=2026-08-20, Friday=2026-08-21, \
             Saturday=2026-08-22, Sunday=2026-08-23, Monday=2026-08-24, Tuesday=2026-08-25"
        );
    }

    fn extraction(title: &str, due_date: Option<&str>, project: Option<&str>) -> RawExtraction {
        RawExtraction {
            title: title.to_string(),
            due_date: due_date.map(str::to_string),
            project: project.map(str::to_string),
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

    fn chat_action(kind: &str, task_id: &str) -> RawChatAction {
        RawChatAction {
            kind: kind.to_string(),
            task_id: Some(task_id.to_string()),
            due_date: None,
            project: None,
            title: None,
            recurrence: None,
        }
    }

    fn project_chat_action(kind: &str) -> RawChatAction {
        RawChatAction {
            kind: kind.to_string(),
            task_id: None,
            due_date: None,
            project: None,
            title: None,
            recurrence: None,
        }
    }

    #[test]
    fn parses_a_set_due_date_action() {
        let id = TaskId::new();
        let mut raw = chat_action("set_due_date", &id.0.to_string());
        raw.due_date = Some("2026-01-05".to_string());
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::SetDueDate {
                task_id: id,
                due_date: NaiveDate::from_ymd_opt(2026, 1, 5).unwrap(),
            }
        );
    }

    #[test]
    fn parses_a_complete_task_action() {
        let id = TaskId::new();
        let action = chat_action("complete_task", &id.0.to_string())
            .into_chat_action()
            .unwrap();
        assert_eq!(action, ChatAction::CompleteTask { task_id: id });
    }

    #[test]
    fn parses_a_move_to_project_action() {
        let id = TaskId::new();
        let mut raw = chat_action("move_to_project", &id.0.to_string());
        raw.project = Some("Groceries".to_string());
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::MoveToProject {
                task_id: id,
                project: "Groceries".to_string(),
            }
        );
    }

    #[test]
    fn rejects_an_unparsable_task_id() {
        let err = chat_action("complete_task", "not-a-uuid")
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("unparsable task_id"));
    }

    #[test]
    fn rejects_an_unknown_action_type() {
        let id = TaskId::new();
        let err = chat_action("archive_task", &id.0.to_string())
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("unknown action type"));
    }

    #[test]
    fn rejects_a_set_due_date_action_missing_a_date() {
        let id = TaskId::new();
        let err = chat_action("set_due_date", &id.0.to_string())
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("missing a due_date"));
    }

    #[test]
    fn rejects_a_move_to_project_action_missing_a_project() {
        let id = TaskId::new();
        let err = chat_action("move_to_project", &id.0.to_string())
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("missing a project"));
    }

    #[test]
    fn parses_a_create_project_action() {
        let mut raw = project_chat_action("create_project");
        raw.project = Some("Taxes".to_string());
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::CreateProject {
                name: "Taxes".to_string(),
            }
        );
    }

    #[test]
    fn parses_a_delete_project_action() {
        let mut raw = project_chat_action("delete_project");
        raw.project = Some("Taxes".to_string());
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::DeleteProject {
                project: "Taxes".to_string(),
            }
        );
    }

    #[test]
    fn rejects_a_create_project_action_missing_a_name() {
        let err = project_chat_action("create_project")
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("missing a project"));
    }

    #[test]
    fn parses_a_full_chat_reply() {
        let id = TaskId::new();
        let raw = RawChatReply {
            reply: "Done.".to_string(),
            actions: vec![chat_action("complete_task", &id.0.to_string())],
        };
        let reply = raw.into_chat_reply(&[]);
        assert_eq!(reply.reply, "Done.");
        assert_eq!(
            reply.actions,
            vec![ChatAction::CompleteTask { task_id: id }]
        );
        assert!(reply.parse_failures.is_empty());
    }

    #[test]
    fn a_malformed_action_is_reported_without_dropping_the_reply_or_other_actions() {
        let id = TaskId::new();
        let mut broken = project_chat_action("create_project");
        broken.project = None; // missing the required field
        let raw = RawChatReply {
            reply: "Rolled the overdue task and made a new project.".to_string(),
            actions: vec![chat_action("complete_task", &id.0.to_string()), broken],
        };
        let reply = raw.into_chat_reply(&[]);
        assert_eq!(
            reply.reply,
            "Rolled the overdue task and made a new project."
        );
        assert_eq!(
            reply.actions,
            vec![ChatAction::CompleteTask { task_id: id }]
        );
        assert_eq!(reply.parse_failures.len(), 1);
        assert!(reply.parse_failures[0].contains("missing a project"));
    }

    #[test]
    fn recovers_a_create_project_action_missing_its_name_from_a_quoted_reply() {
        let raw = RawChatReply {
            reply: "Created new project 'Wei-wu'.".to_string(),
            actions: vec![project_chat_action("create_project")],
        };
        let reply = raw.into_chat_reply(&[]);
        assert_eq!(
            reply.actions,
            vec![ChatAction::CreateProject {
                name: "Wei-wu".to_string(),
            }]
        );
        assert!(reply.parse_failures.is_empty());
    }

    #[test]
    fn recovers_a_delete_project_action_missing_its_name_from_a_quoted_reply() {
        let raw = RawChatReply {
            reply: "Deleted the \"Wei-wu\" project.".to_string(),
            actions: vec![project_chat_action("delete_project")],
        };
        let reply = raw.into_chat_reply(&[]);
        assert_eq!(
            reply.actions,
            vec![ChatAction::DeleteProject {
                project: "Wei-wu".to_string(),
            }]
        );
        assert!(reply.parse_failures.is_empty());
    }

    #[test]
    fn does_not_guess_a_project_name_when_the_reply_has_no_quoted_text() {
        let raw = RawChatReply {
            reply: "Created the new project.".to_string(),
            actions: vec![project_chat_action("create_project")],
        };
        let reply = raw.into_chat_reply(&[]);
        assert!(reply.actions.is_empty());
        assert_eq!(reply.parse_failures.len(), 1);
    }

    #[test]
    fn recovers_a_delete_project_action_from_an_unquoted_but_known_project_name() {
        // Reproduces an observed failure: the model's reply names the
        // project in plain prose, no quotes at all, and leaves the
        // delete_project action's `project` field empty.
        let raw = RawChatReply {
            reply: "Deleted the Recurring project. Its tasks have been moved to the Inbox."
                .to_string(),
            actions: vec![project_chat_action("delete_project")],
        };
        let known = vec!["Recurring".to_string(), "Wei-wu".to_string()];
        let reply = raw.into_chat_reply(&known);
        assert_eq!(
            reply.actions,
            vec![ChatAction::DeleteProject {
                project: "Recurring".to_string(),
            }]
        );
        assert!(reply.parse_failures.is_empty());
    }

    #[test]
    fn does_not_guess_a_delete_project_name_when_no_known_project_is_mentioned() {
        let raw = RawChatReply {
            reply: "Deleted it.".to_string(),
            actions: vec![project_chat_action("delete_project")],
        };
        let known = vec!["Recurring".to_string(), "Wei-wu".to_string()];
        let reply = raw.into_chat_reply(&known);
        assert!(reply.actions.is_empty());
        assert_eq!(reply.parse_failures.len(), 1);
    }

    #[test]
    fn does_not_guess_a_create_project_name_from_the_known_project_list() {
        // create_project names a *new* project, so matching against the
        // existing list would never be the right recovery for it — only
        // quoting should ever work here.
        let raw = RawChatReply {
            reply: "Created a project like Recurring but for one-offs.".to_string(),
            actions: vec![project_chat_action("create_project")],
        };
        let known = vec!["Recurring".to_string()];
        let reply = raw.into_chat_reply(&known);
        assert!(reply.actions.is_empty());
        assert_eq!(reply.parse_failures.len(), 1);
    }

    #[test]
    fn does_not_guess_a_project_name_when_the_reply_has_multiple_quoted_spans() {
        let raw = RawChatReply {
            reply: "Created 'Wei-wu', separate from 'Wei-mu'.".to_string(),
            actions: vec![project_chat_action("create_project")],
        };
        let reply = raw.into_chat_reply(&[]);
        assert!(reply.actions.is_empty());
        assert_eq!(reply.parse_failures.len(), 1);
    }

    #[test]
    fn parses_a_delete_task_action() {
        let id = TaskId::new();
        let action = chat_action("delete_task", &id.0.to_string())
            .into_chat_action()
            .unwrap();
        assert_eq!(action, ChatAction::DeleteTask { task_id: id });
    }

    #[test]
    fn rejects_a_delete_task_action_missing_a_task_id() {
        let err = project_chat_action("delete_task")
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("missing a task_id"));
    }

    #[test]
    fn parses_a_set_recurrence_action() {
        let id = TaskId::new();
        let mut raw = chat_action("set_recurrence", &id.0.to_string());
        raw.recurrence = Some(RawRecurrence {
            interval: 2,
            unit: "weeks".to_string(),
        });
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::SetRecurrence {
                task_id: id,
                recurrence: Recurrence {
                    interval: 2,
                    unit: RecurrenceUnit::Weeks,
                },
            }
        );
    }

    #[test]
    fn rejects_a_set_recurrence_action_missing_a_recurrence() {
        let id = TaskId::new();
        let err = chat_action("set_recurrence", &id.0.to_string())
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("missing a recurrence"));
    }

    #[test]
    fn parses_a_clear_recurrence_action() {
        let id = TaskId::new();
        let action = chat_action("clear_recurrence", &id.0.to_string())
            .into_chat_action()
            .unwrap();
        assert_eq!(action, ChatAction::ClearRecurrence { task_id: id });
    }

    #[test]
    fn parses_a_full_create_task_action() {
        let mut raw = project_chat_action("create_task");
        raw.title = Some("Review Rocket Money".to_string());
        raw.due_date = Some("2026-08-24".to_string());
        raw.project = Some("Habits".to_string());
        raw.recurrence = Some(RawRecurrence {
            interval: 1,
            unit: "weeks".to_string(),
        });
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::CreateTask {
                task: ParsedTask {
                    title: "Review Rocket Money".to_string(),
                    due_date: Some(NaiveDate::from_ymd_opt(2026, 8, 24).unwrap()),
                    project: Some("Habits".to_string()),
                    recurrence: Some(Recurrence {
                        interval: 1,
                        unit: RecurrenceUnit::Weeks,
                    }),
                },
            }
        );
    }

    #[test]
    fn parses_a_minimal_create_task_action() {
        let mut raw = project_chat_action("create_task");
        raw.title = Some("buy milk".to_string());
        let action = raw.into_chat_action().unwrap();
        assert_eq!(
            action,
            ChatAction::CreateTask {
                task: ParsedTask {
                    title: "buy milk".to_string(),
                    due_date: None,
                    project: None,
                    recurrence: None,
                },
            }
        );
    }

    #[test]
    fn rejects_a_create_task_action_missing_a_title() {
        let err = project_chat_action("create_task")
            .into_chat_action()
            .unwrap_err();
        assert!(err.contains("missing a title"));
    }
}
