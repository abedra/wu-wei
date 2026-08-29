use std::sync::mpsc::{Receiver, TryRecvError};

use chrono::{Datelike, Duration, Local, NaiveDate, Utc, Weekday};
use rusqlite::Connection;
use uuid::Uuid;

use crate::calendar::{self, CalendarEvent, GoogleCalendarConfig, GoogleTokenSet};
use crate::db;
use crate::db::error::DbResult;
use crate::db::{project_repo, settings_repo, task_repo};
use crate::db_bootstrap;
use crate::domain::project::{Project, ProjectId, ProjectKind, ProjectStatus};
#[cfg(test)]
use crate::domain::task::RecurrenceUnit;
use crate::domain::task::{Recurrence, Task, TaskId};
use crate::llm::{
    self, ChatAction, ChatCalendarEventSummary, ChatCompletedTaskSummary, ChatContext, ChatReply,
    ChatRole, ChatTaskSummary, ChatTurn, LlmConfig, ParsedTask, PromptContext, ProviderKind,
};
use crate::sync::{self, SyncSummary};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perspective {
    Inbox,
    Today,
    Overdue,
    /// Every open task across every project (and the Inbox) — a weekly-review
    /// perspective for scanning everything at once and deciding what needs a
    /// new due date, independent of which project it happens to live in.
    Review,
    Completed,
    Project(ProjectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    None,
    Task(TaskId),
    Project(ProjectId),
}

/// How `visible_tasks` is ordered on top of whatever a perspective's own
/// query already returns. `Natural` leaves the per-perspective order (see
/// `AppState::refresh_visible_tasks`) untouched; the other variants apply an
/// in-memory re-sort so they work the same regardless of perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskSortKey {
    #[default]
    Natural,
    DueDate,
    Project,
    Estimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    #[default]
    Ascending,
    Descending,
}

impl SortDirection {
    fn toggled(self) -> Self {
        match self {
            SortDirection::Ascending => SortDirection::Descending,
            SortDirection::Descending => SortDirection::Ascending,
        }
    }
}

/// The name shown for a task's project column: the project's own name, or
/// "Inbox" for a task that isn't assigned to one. Shared by the task list's
/// project column and its project-name sort so both agree on what a task's
/// project "is".
pub fn project_display_name(project_id: Option<ProjectId>, projects: &[Project]) -> String {
    match project_id {
        None => "Inbox".to_string(),
        Some(pid) => projects
            .iter()
            .find(|p| p.id == pid)
            .map(|p| p.name.clone())
            .unwrap_or_default(),
    }
}

/// A small keyboard-driven picker for reassigning a task's project.
/// `highlighted` indexes a virtual list: `0` is Inbox, `i + 1` is `AppState.projects[i]`.
pub struct ProjectPickerState {
    pub task_id: TaskId,
    pub highlighted: usize,
}

/// A small keyboard-driven picker for setting a task's due date.
/// `highlighted` indexes into [`due_date_picker_options`].
pub struct DueDatePickerState {
    pub task_id: TaskId,
    pub highlighted: usize,
    /// The free-text "type a date" field (e.g. "next friday", "in 3 weeks"),
    /// resolved to a real date by the LLM — see
    /// `AppState::submit_due_date_picker_text`.
    pub text_input: String,
    /// Set while the typed phrase is being resolved on a background thread;
    /// polled in `AppState::poll_due_date_picker`. Dropping the picker (e.g.
    /// Esc) drops this and abandons the in-flight request.
    pub ai_pending: Option<Receiver<Result<NaiveDate, String>>>,
    /// The last typed-date attempt's failure ("couldn't read a date from
    /// that", no provider configured, ...), shown inline in the picker.
    pub ai_error: Option<String>,
}

/// A small keyboard-driven picker for setting a task's time estimate.
/// `highlighted` indexes into [`estimate_picker_options`].
pub struct EstimatePickerState {
    pub task_id: TaskId,
    pub highlighted: usize,
    /// Free-text minutes/hours field, e.g. "90", "45m", "1h30m" — parsed
    /// locally by [`parse_estimate`], no AI involved (unlike the due-date
    /// picker's typed field).
    pub text_input: String,
    /// The last typed-estimate attempt's failure, shown inline in the picker.
    pub error: Option<String>,
}

/// Quick estimate choices offered by the picker, paired with the whole
/// minutes each resolves to (`None` clears the estimate).
pub fn estimate_picker_options() -> Vec<(String, Option<i64>)> {
    vec![
        ("No Estimate".to_string(), None),
        ("15 min".to_string(), Some(15)),
        ("30 min".to_string(), Some(30)),
        ("45 min".to_string(), Some(45)),
        ("1 hour".to_string(), Some(60)),
        ("1.5 hours".to_string(), Some(90)),
        ("2 hours".to_string(), Some(120)),
        ("3 hours".to_string(), Some(180)),
        ("4 hours".to_string(), Some(240)),
    ]
}

/// Parses a short estimate phrase into whole minutes. Accepts a bare number
/// as minutes ("90"), an `h`/`m` combo ("1h", "1h30m", "45m", "2 hours"),
/// or decimal hours ("1.5h"). Returns `None` when nothing sensible can be
/// read or the result isn't positive.
pub fn parse_estimate(text: &str) -> Option<i64> {
    let s: String = text
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if s.is_empty() {
        return None;
    }

    // Bare number → minutes.
    if let Ok(n) = s.parse::<f64>() {
        let m = n.round() as i64;
        return (m > 0).then_some(m);
    }

    let mut minutes = 0.0_f64;
    let mut matched = false;
    let mut rest = s.as_str();

    if let Some(idx) = rest.find('h') {
        let hours: f64 = rest[..idx].parse().ok()?;
        minutes += hours * 60.0;
        matched = true;
        // Drop the rest of a spelled-out unit ("hr", "hours") before the
        // trailing minutes portion.
        rest = rest[idx + 1..].trim_start_matches(|c: char| c.is_alphabetic());
    }

    if !rest.is_empty() {
        let digits: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if digits.is_empty() {
            if !matched {
                return None;
            }
        } else {
            minutes += digits.parse::<f64>().ok()?;
            matched = true;
        }
    }

    let m = minutes.round() as i64;
    (matched && m > 0).then_some(m)
}

/// The next Saturday from `today`, or `today` itself if it's already a
/// Saturday or Sunday.
fn this_weekend(today: NaiveDate) -> NaiveDate {
    if matches!(today.weekday(), Weekday::Sat | Weekday::Sun) {
        return today;
    }
    today + Duration::days(days_until(today.weekday(), Weekday::Sat))
}

/// The next Monday after `today` (always strictly in the future, even if
/// `today` is itself a Monday).
fn next_week(today: NaiveDate) -> NaiveDate {
    let days = days_until(today.weekday(), Weekday::Mon);
    today + Duration::days(if days == 0 { 7 } else { days })
}

fn days_until(from: Weekday, target: Weekday) -> i64 {
    (target.num_days_from_monday() as i64 - from.num_days_from_monday() as i64 + 7) % 7
}

/// Quick due-date choices offered by the picker, paired with the concrete
/// date each resolves to (`None` clears the due date).
pub fn due_date_picker_options(today: NaiveDate) -> Vec<(String, Option<NaiveDate>)> {
    let tomorrow = today + Duration::days(1);
    let weekend = this_weekend(today);
    let next_mon = next_week(today);
    vec![
        ("No Due Date".to_string(), None),
        (format!("Today - {}", today.format("%Y-%m-%d")), Some(today)),
        (
            format!("Tomorrow - {}", tomorrow.format("%Y-%m-%d")),
            Some(tomorrow),
        ),
        (
            format!("This Weekend - {}", weekend.format("%Y-%m-%d")),
            Some(weekend),
        ),
        (
            format!("Next Week - {}", next_mon.format("%Y-%m-%d")),
            Some(next_mon),
        ),
    ]
}

pub struct TaskEditBuffer {
    pub id: TaskId,
    pub title: String,
    pub notes: String,
    pub project_id: Option<ProjectId>,
    pub due_date: Option<NaiveDate>,
    pub defer_date: Option<NaiveDate>,
    pub completed: bool,
    pub completed_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
    pub estimated_minutes: Option<i64>,
    pub recurrence: Option<Recurrence>,
}

impl TaskEditBuffer {
    fn from_task(task: &Task) -> Self {
        TaskEditBuffer {
            id: task.id,
            title: task.title.clone(),
            notes: task.notes.clone(),
            project_id: task.project_id,
            due_date: task.due_date,
            defer_date: task.defer_date,
            completed: task.completed,
            completed_at: task.completed_at,
            created_at: task.created_at,
            estimated_minutes: task.estimated_minutes,
            recurrence: task.recurrence,
        }
    }
}

/// Editable copy of the Settings screen's fields (see `ui::settings`),
/// committed as a whole by `AppState::save_settings` or thrown away by
/// `AppState::close_settings` — same draft-buffer shape as
/// `TaskEditBuffer`/`ProjectEditBuffer`, except commit is an explicit Save
/// rather than live-as-you-type: unlike those two, a half-typed value here
/// (a garbage db path, a blank API key) shouldn't take effect immediately.
#[derive(Debug, Clone)]
pub struct SettingsDraft {
    pub llm_provider: ProviderKind,
    pub llm_api_key: String,
    /// Whether the API key field currently shows plain text instead of
    /// masking it — toggled by the eye button in `ui::settings`. Transient
    /// UI state, not persisted and not part of what Save writes out.
    pub show_api_key: bool,
    pub llm_base_url: String,
    pub llm_model: String,
    pub db_path: String,
    pub sync_folder_path: String,
    pub gcal_client_id: String,
    pub gcal_client_secret: String,
    /// Same transient, not-persisted purpose as `show_api_key`, for the
    /// client secret field.
    pub show_gcal_client_secret: bool,
    /// How often the Today view auto-refreshes calendar events, in minutes.
    /// Seeded (and clamped) via `calendar::refresh_minutes`, persisted as
    /// `gcal_refresh_minutes` on Save.
    pub gcal_refresh_minutes: u64,
    /// Whether `settings` already has a Google refresh token saved —
    /// display-only (drives "Connect" vs "Connected" in `ui::settings`),
    /// never itself written back on Save; only `AppState::poll_google_auth`/
    /// `disconnect_google_calendar` ever touch the actual token settings.
    pub gcal_connected: bool,
}

/// `Connection::path` returns `None` for an in-memory or otherwise
/// path-less database — the same fallback used whether reading the current
/// path (`SettingsDraft::load`) or deciding whether Settings' db-path field
/// actually changed (`AppState::save_settings`), so the two never disagree
/// and trigger a spurious `switch_database` on every Save.
fn current_db_path(conn: &Connection) -> String {
    // `Connection::path` returns `Some("")` — not `None` — for an
    // in-memory or temporary database, so an emptiness check is needed,
    // not just an `Option` one (same gotcha as `AppState::run_sync`'s
    // identical fallback).
    conn.path()
        .filter(|p| !p.is_empty())
        .unwrap_or("wu_wei.db")
        .to_string()
}

/// The calendar auto-refresh interval as a `Duration`, from the raw
/// `settings` map — wraps `calendar::refresh_minutes` (which handles the
/// default and clamping) so both `AppState::new` and `save_settings` derive
/// it the same way.
fn calendar_refresh_interval(settings: &std::collections::HashMap<String, String>) -> Duration {
    Duration::minutes(calendar::refresh_minutes(settings) as i64)
}

/// Drops repeated entries, keeping the first occurrence's position — used
/// for the chat panel's failure list, since a batch action gone wrong (e.g.
/// the model mangling the same task id across several actions) otherwise
/// repeats the identical error once per action instead of once per distinct
/// problem.
fn dedup_keep_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.clone()))
        .collect()
}

impl SettingsDraft {
    /// Seeds the draft from whatever's actually in effect right now: a
    /// non-blank value already saved to `settings` wins, otherwise it falls
    /// back to the live `LlmConfig` (itself env-derived when nothing's been
    /// saved yet) so the screen shows what's really active rather than
    /// starting blank. A *blank* saved value is treated the same as no
    /// saved value at all — otherwise a Settings screen opened once with an
    /// empty field (e.g. before the live config had loaded) would persist
    /// that blank on Save and permanently shadow the env-derived value on
    /// every future load, even though nothing was ever deliberately cleared.
    fn load(conn: &Connection, current: Option<&LlmConfig>) -> Self {
        let settings = settings_repo::get_all(conn).unwrap_or_default();
        let non_blank = |key: &str| {
            settings
                .get(key)
                .map(String::as_str)
                .filter(|s| !s.trim().is_empty())
        };

        let provider_str = non_blank("llm_provider").or(current.map(|c| match c.provider {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
        }));
        let llm_provider = match provider_str {
            Some("anthropic") => ProviderKind::Anthropic,
            _ => ProviderKind::OpenAi,
        };

        let field = |key: &str, fallback: Option<&String>| -> String {
            non_blank(key)
                .map(str::to_string)
                .or_else(|| fallback.cloned())
                .unwrap_or_default()
        };

        SettingsDraft {
            llm_provider,
            llm_api_key: field("llm_api_key", current.map(|c| &c.api_key)),
            show_api_key: false,
            llm_base_url: field("llm_base_url", current.map(|c| &c.base_url)),
            llm_model: field("llm_model", current.map(|c| &c.model)),
            db_path: current_db_path(conn),
            sync_folder_path: settings
                .get("sync_folder_path")
                .cloned()
                .unwrap_or_default(),
            gcal_client_id: field("gcal_client_id", None),
            gcal_client_secret: field("gcal_client_secret", None),
            show_gcal_client_secret: false,
            gcal_connected: non_blank("gcal_refresh_token").is_some(),
            gcal_refresh_minutes: calendar::refresh_minutes(&settings),
        }
    }
}

pub struct ProjectEditBuffer {
    pub id: ProjectId,
    pub name: String,
    pub notes: String,
    pub status: ProjectStatus,
    pub kind: ProjectKind,
}

impl ProjectEditBuffer {
    fn from_project(project: &Project) -> Self {
        ProjectEditBuffer {
            id: project.id,
            name: project.name.clone(),
            notes: project.notes.clone(),
            status: project.status,
            kind: project.kind,
        }
    }
}

/// One action the AI proposed but hasn't applied yet, paired with the
/// human-readable description shown in the confirmation prompt (see
/// `AppState::pending_chat_actions`).
pub struct PendingChatAction {
    pub action: ChatAction,
    pub description: String,
}

pub struct AppState {
    pub conn: Connection,
    pub projects: Vec<Project>,
    pub visible_tasks: Vec<Task>,
    pub perspective: Perspective,
    /// The calendar date `visible_tasks` was last computed against — not
    /// user-facing state, just how `refresh_if_date_changed` notices the
    /// wall clock has crossed midnight since the last refresh (e.g. the app
    /// was left open on Today overnight) and knows to requery rather than
    /// silently going stale. `app.rs` calls it once per frame, alongside a
    /// periodic repaint request that keeps frames happening even when the
    /// app is otherwise idle.
    pub last_seen_date: NaiveDate,
    /// User-chosen override applied on top of the current perspective's
    /// natural order (see `TaskSortKey`), toggled by clicking the "Due" or
    /// "Project" column header in `ui::task_list`.
    pub sort_key: TaskSortKey,
    pub sort_direction: SortDirection,
    pub selection: Selection,
    /// Keyboard cursor over `visible_tasks`, moved by the Up/Down arrows.
    /// Independent of `selection`: moving it does not open the detail panel
    /// (mirrors OmniFocus, where arrow keys move the row cursor without
    /// forcing the inspector open). `Enter` toggles the detail panel for
    /// whichever task this points at; Space/delete/move-to-project all
    /// act on it too, whether or not the detail panel happens to be open.
    pub highlighted_task: Option<TaskId>,
    pub task_edit_buffer: Option<TaskEditBuffer>,
    pub project_edit_buffer: Option<ProjectEditBuffer>,
    pub quick_entry_buffer: String,
    /// Whether the quick-capture popup is open (toggled by Cmd+N). It's a
    /// floating window rather than a permanent panel, so it never sits in the
    /// normal Tab order when closed.
    pub quick_capture_open: bool,
    /// One-shot flag consumed by `ui::quick_capture::draw` right after it
    /// adds the field, so the actual `request_focus` call happens in the
    /// same `draw` pass that creates the widget (see `ui::ai_chat`'s
    /// `chat_focus_requested` for the same pattern) rather than from
    /// `ui::shortcuts` a step earlier — requesting focus by id before the
    /// widget for that frame even exists proved unreliable in practice.
    pub quick_capture_focus_pending: bool,
    /// Whether the new-project popup is open (toggled by Cmd+Shift+N). Same
    /// floating-window shape as `quick_capture_open`.
    pub new_project_popup_open: bool,
    /// Same one-shot purpose as `quick_capture_focus_pending`, for
    /// `ui::new_project::draw`.
    pub new_project_focus_pending: bool,
    pub new_project_name: String,
    pub project_picker: Option<ProjectPickerState>,
    pub due_date_picker: Option<DueDatePickerState>,
    pub estimate_picker: Option<EstimatePickerState>,
    /// Whether keyboard control is currently on the sidebar (see
    /// [`AppState::focus_sidebar`]): Up/Down step through perspectives instead
    /// of the task list, and other task shortcuts (Space, M, D, ...) stand down.
    pub sidebar_focused: bool,
    /// Whether the right-hand detail panel is currently shown. Automatically
    /// opens on a new selection and closes when the selection is cleared
    /// (mirrors OmniFocus: the inspector only makes sense with something to
    /// inspect), but can also be toggled directly regardless of selection
    /// (the info button — see `AppState::toggle_detail_panel`).
    pub detail_panel_open: bool,
    /// Whether the "Archive Completed" confirmation is open (see
    /// `ui::archive_confirm`, triggered by a button shown only in the
    /// Completed view). Deleting completed tasks is permanent — this is the
    /// one bulk, irreversible action reachable from a button rather than a
    /// single-item Cmd+Backspace, hence the extra confirmation step.
    pub archive_confirm_open: bool,
    pub error_message: Option<String>,
    /// The Settings screen's draft, if it's currently open (toggled by
    /// Cmd+, — see `ui::shortcuts`/`ui::settings`). `None` when closed; a
    /// floating window like the other popups, not part of the normal Tab
    /// order.
    pub settings: Option<SettingsDraft>,
    /// `None` when no provider is configured (see `LlmConfig::resolve`) —
    /// AI-assisted capture is simply unavailable, not an error, until set up.
    pub llm_config: Option<LlmConfig>,
    /// Set while a background AI-parse request is in flight; polled once per
    /// frame in `poll_llm`. Dropping it (e.g. on cancel) discards the reply.
    pub llm_pending: Option<Receiver<Result<ParsedTask, String>>>,
    pub llm_busy: bool,
    /// The AI chat panel's conversation so far, shown in the transcript and
    /// resent as context on every turn. Ephemeral — not persisted to the DB,
    /// cleared on restart.
    pub chat_history: Vec<ChatTurn>,
    pub chat_input: String,
    /// Set while a background chat request is in flight; polled once per
    /// frame in `poll_chat`.
    pub chat_pending: Option<Receiver<Result<ChatReply, String>>>,
    pub chat_busy: bool,
    /// Set by `poll_chat` when a request finishes (reply or error); consumed
    /// by `ui::ai_chat::draw`, which requests keyboard focus back on the
    /// input field and clears it, so the next message can be typed
    /// immediately without reaching for the mouse.
    pub chat_focus_requested: bool,
    /// Actions the AI proposed in its last reply that haven't been applied
    /// yet — the model is unreliable about only proposing a change when one
    /// was actually asked for (see `AppState::receive_chat_reply`), so
    /// nothing in this list touches the database until the user explicitly
    /// confirms via `confirm_pending_chat_actions` (or discards it via
    /// `discard_pending_chat_actions`). Empty means nothing is pending.
    pub pending_chat_actions: Vec<PendingChatAction>,
    /// Set while a background sync run is in flight; polled once per frame
    /// in `poll_sync`. Same shape as `llm_pending`/`llm_busy`.
    pub sync_pending: Option<Receiver<Result<SyncSummary, String>>>,
    pub sync_busy: bool,
    /// The last sync attempt's result or error, shown near the sync button
    /// (see `ui::sidebar`). `None` before the first attempt.
    pub sync_status: Option<String>,
    /// When `maybe_auto_sync` last actually ran one — `None` means it
    /// hasn't yet, which is also what makes it fire on the very first
    /// frame (covers "sync on launch").
    pub last_auto_sync: Option<chrono::DateTime<Utc>>,
    /// Set while the native "Browse…" dialog for Settings' database file
    /// path is open — see `browse_for_db_path`/`poll_db_path_dialog`. `rfd`'s
    /// dialogs block the calling thread, so (like `llm_pending`/`sync_pending`)
    /// this runs on a background thread rather than blocking the UI: doing it
    /// on the UI thread would freeze the whole app's rendering for as long as
    /// the dialog stayed open.
    pub db_path_dialog: Option<Receiver<Option<String>>>,
    /// Same shape as `db_path_dialog`, for Settings' sync folder field.
    pub sync_folder_dialog: Option<Receiver<Option<String>>>,
    /// `None` when Google Calendar hasn't been connected (see
    /// `GoogleCalendarConfig::resolve`) — the "Today's Events" section is
    /// simply not shown, not an error, until the user connects it in
    /// Settings.
    pub google_calendar_config: Option<GoogleCalendarConfig>,
    /// Today's events from the connected calendar, refreshed by
    /// `run_calendar_sync`/`poll_calendar`. Never persisted — re-fetched on
    /// every sync, same "external source of truth, local cache only"
    /// relationship `visible_tasks` has with the database.
    pub calendar_events: Vec<CalendarEvent>,
    /// Set while a background calendar fetch is in flight; polled once per
    /// frame in `poll_calendar`. Same shape as `sync_pending`/`sync_busy`.
    pub calendar_pending: Option<Receiver<Result<Vec<CalendarEvent>, String>>>,
    pub calendar_busy: bool,
    /// The last calendar fetch's error, if any — shown inline in the
    /// "Today's Events" section (see `ui::task_list`). Mirrors `sync_status`:
    /// a dedicated slot rather than the generic `error_message` banner, so a
    /// standing auth problem doesn't re-post to the banner every auto-sync.
    pub calendar_status: Option<String>,
    /// When `maybe_auto_sync_calendar` last actually ran one — `None` means
    /// it hasn't yet, mirrors `last_auto_sync`.
    pub last_calendar_sync: Option<chrono::DateTime<Utc>>,
    /// How long `maybe_auto_sync_calendar` waits between auto-refreshes.
    /// Resolved from the `gcal_refresh_minutes` setting at startup and again
    /// on every `save_settings` (see `calendar::refresh_minutes`).
    pub calendar_refresh_interval: Duration,
    /// Set while the Google OAuth loopback flow (browser + local redirect
    /// listener) is in flight; polled once per frame in `poll_google_auth`.
    pub google_auth_pending: Option<Receiver<Result<GoogleTokenSet, String>>>,
    /// Status/progress text for the OAuth flow, shown under the
    /// Connect/Disconnect button in `ui::settings`.
    pub google_auth_status: Option<String>,
}

impl AppState {
    pub fn new(conn: Connection) -> Self {
        let llm_settings = settings_repo::get_all(&conn).unwrap_or_default();
        let mut state = AppState {
            conn,
            projects: Vec::new(),
            visible_tasks: Vec::new(),
            perspective: Perspective::Inbox,
            last_seen_date: Local::now().date_naive(),
            sort_key: TaskSortKey::default(),
            sort_direction: SortDirection::default(),
            selection: Selection::None,
            highlighted_task: None,
            task_edit_buffer: None,
            project_edit_buffer: None,
            quick_entry_buffer: String::new(),
            quick_capture_open: false,
            quick_capture_focus_pending: false,
            new_project_popup_open: false,
            new_project_focus_pending: false,
            new_project_name: String::new(),
            project_picker: None,
            due_date_picker: None,
            estimate_picker: None,
            sidebar_focused: false,
            detail_panel_open: false,
            archive_confirm_open: false,
            error_message: None,
            settings: None,
            llm_config: LlmConfig::resolve(&llm_settings),
            llm_pending: None,
            llm_busy: false,
            chat_history: Vec::new(),
            chat_input: String::new(),
            chat_pending: None,
            chat_busy: false,
            chat_focus_requested: false,
            pending_chat_actions: Vec::new(),
            sync_pending: None,
            sync_busy: false,
            sync_status: None,
            last_auto_sync: None,
            db_path_dialog: None,
            sync_folder_dialog: None,
            google_calendar_config: GoogleCalendarConfig::resolve(&llm_settings),
            calendar_events: Vec::new(),
            calendar_pending: None,
            calendar_busy: false,
            calendar_status: None,
            last_calendar_sync: None,
            calendar_refresh_interval: calendar_refresh_interval(&llm_settings),
            google_auth_pending: None,
            google_auth_status: None,
        };
        state.refresh_projects();
        state.refresh_visible_tasks();
        state
    }

    pub fn set_perspective(&mut self, p: Perspective) {
        self.perspective = p;
        self.clear_selection();
        self.highlighted_task = None;
        self.sidebar_focused = false;
        self.refresh_visible_tasks();
        if p == Perspective::Today {
            // Fires immediately on first visit (`last_calendar_sync` is
            // still `None`) rather than waiting out the auto-sync interval —
            // see `maybe_auto_sync_calendar`.
            self.maybe_auto_sync_calendar();
        }
    }

    /// Clears whatever is selected and its edit buffer, closing the detail
    /// panel along with it (see `AppState::detail_panel_open`).
    fn clear_selection(&mut self) {
        self.selection = Selection::None;
        self.task_edit_buffer = None;
        self.project_edit_buffer = None;
        self.detail_panel_open = false;
    }

    /// The info-button toggle: shows or hides the detail panel regardless of
    /// whether anything is currently selected.
    pub fn toggle_detail_panel(&mut self) {
        self.detail_panel_open = !self.detail_panel_open;
    }

    /// Every sidebar row's perspective, in the exact order `ui::sidebar::draw`
    /// renders them. The single source of truth both use, so keyboard
    /// navigation indices always line up with what's on screen.
    pub fn sidebar_entries(&self) -> Vec<Perspective> {
        let mut entries = vec![
            Perspective::Inbox,
            Perspective::Today,
            Perspective::Overdue,
            Perspective::Review,
            Perspective::Completed,
        ];
        entries.extend(self.projects.iter().map(|p| Perspective::Project(p.id)));
        entries
    }

    /// Enters sidebar keyboard-navigation mode on a specific row (a click, or
    /// keyboard focus landing there via Tab).
    pub fn focus_sidebar(&mut self, index: usize) {
        let entries = self.sidebar_entries();
        let Some(&perspective) = entries.get(index) else {
            return;
        };
        self.set_perspective(perspective);
        self.sidebar_focused = true;
        // Navigating to a sidebar item immediately places the keyboard inside
        // that item's content (its first task, if it shows one) — task
        // shortcuts (Space, Enter, M, D, ...) work right away, with no extra
        // step to "enter" the content. Up/Down still browse the sidebar itself
        // until Tab hands them over to the list (see `exit_sidebar_focus`).
        self.move_highlight(0);
    }

    /// Moves the current perspective by `delta` rows within the sidebar's
    /// flattened entry list (negative moves up), switching to it immediately
    /// so the content pane always previews whatever is highlighted.
    pub fn move_sidebar_highlight(&mut self, delta: i32) {
        let entries = self.sidebar_entries();
        if entries.is_empty() {
            return;
        }
        let current = entries
            .iter()
            .position(|&p| p == self.perspective)
            .unwrap_or(0);
        let next = (current as i32 + delta).clamp(0, entries.len() as i32 - 1) as usize;
        self.set_perspective(entries[next]);
        self.sidebar_focused = true;
        self.move_highlight(0);
    }

    /// Leaves sidebar keyboard-navigation mode, handing Up/Down over to the
    /// content list itself (rather than just its already-highlighted first
    /// item — see `focus_sidebar`).
    pub fn exit_sidebar_focus(&mut self) {
        self.sidebar_focused = false;
    }

    pub fn refresh_visible_tasks(&mut self) {
        let today = Local::now().date_naive();
        let result = match self.perspective {
            Perspective::Inbox => task_repo::list_inbox(&self.conn),
            Perspective::Today => task_repo::list_today(&self.conn, today),
            Perspective::Overdue => task_repo::list_overdue(&self.conn, today),
            Perspective::Review => task_repo::list_open(&self.conn),
            Perspective::Completed => task_repo::list_completed(&self.conn),
            Perspective::Project(id) => task_repo::list_by_project(&self.conn, id),
        };
        self.visible_tasks = self.unwrap_or_report(result, Vec::new());
        self.sort_visible_tasks();
        if let Some(id) = self.highlighted_task
            && !self.visible_tasks.iter().any(|t| t.id == id)
        {
            self.highlighted_task = None;
        }
    }

    /// Picks `key` as the sort applied on top of the current perspective
    /// (see `TaskSortKey`): clicking the same header again flips direction,
    /// clicking a different one switches to it. Due date defaults to
    /// descending on first click — the common case of wanting the furthest-
    /// out work at the top with undated tasks trailing behind everything —
    /// while project defaults to ascending (alphabetical).
    pub fn set_sort(&mut self, key: TaskSortKey) {
        if self.sort_key == key {
            self.sort_direction = self.sort_direction.toggled();
        } else {
            self.sort_key = key;
            self.sort_direction = match key {
                TaskSortKey::DueDate => SortDirection::Descending,
                TaskSortKey::Natural | TaskSortKey::Project | TaskSortKey::Estimate => {
                    SortDirection::Ascending
                }
            };
        }
        self.sort_visible_tasks();
    }

    /// Re-sorts `visible_tasks` in place per `sort_key`/`sort_direction`,
    /// without re-querying the database. `Natural` leaves the perspective
    /// query's own order alone. `DueDate` always sorts tasks with no due
    /// date last, regardless of direction — an undated task has nothing to
    /// compare, so there's no sense in which it's "later" or "earlier" than
    /// a dated one.
    fn sort_visible_tasks(&mut self) {
        match self.sort_key {
            TaskSortKey::Natural => {}
            TaskSortKey::DueDate => {
                let direction = self.sort_direction;
                self.visible_tasks
                    .sort_by(|a, b| match (a.due_date, b.due_date) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (Some(x), Some(y)) => match direction {
                            SortDirection::Ascending => x.cmp(&y),
                            SortDirection::Descending => y.cmp(&x),
                        },
                    });
            }
            TaskSortKey::Estimate => {
                let direction = self.sort_direction;
                // Like `DueDate`, a task with no estimate has nothing to
                // compare, so it always sorts last regardless of direction.
                self.visible_tasks.sort_by(|a, b| {
                    match (a.estimated_minutes, b.estimated_minutes) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (Some(x), Some(y)) => match direction {
                            SortDirection::Ascending => x.cmp(&y),
                            SortDirection::Descending => y.cmp(&x),
                        },
                    }
                });
            }
            TaskSortKey::Project => {
                let direction = self.sort_direction;
                let projects = &self.projects;
                self.visible_tasks.sort_by(|a, b| {
                    let name_a = project_display_name(a.project_id, projects);
                    let name_b = project_display_name(b.project_id, projects);
                    match direction {
                        SortDirection::Ascending => name_a.cmp(&name_b),
                        SortDirection::Descending => name_b.cmp(&name_a),
                    }
                });
            }
        }
    }

    /// Catches the wall clock crossing midnight while the app just sat open
    /// (e.g. left on Today overnight): without this, `visible_tasks` stays
    /// exactly what it was at the last refresh, since nothing else re-queries
    /// it on its own. Cheap to call every frame — it's a date comparison
    /// that only does real work on the rare frame where the date actually
    /// changed.
    pub fn refresh_if_date_changed(&mut self) {
        let today = Local::now().date_naive();
        if today != self.last_seen_date {
            self.last_seen_date = today;
            self.refresh_visible_tasks();
        }
    }

    /// The stable per-installation id used to name this device's snapshot
    /// file (see `sync.rs`). Resolved from `settings` (key
    /// `sync_device_id`), generating and persisting one on first use.
    fn sync_device_id(&self) -> String {
        if let Ok(settings) = settings_repo::get_all(&self.conn)
            && let Some(id) = settings.get("sync_device_id")
        {
            return id.clone();
        }
        let id = Uuid::new_v4().to_string();
        let _ = settings_repo::set(&self.conn, "sync_device_id", &id);
        id
    }

    /// Kicks off a sync run on a background thread (see
    /// `sync::run_async`) — safe to call anytime; it's a no-op (with an
    /// explanatory `sync_status`) if no folder is configured, one's
    /// already in flight, or this database has no file path to hand a
    /// second connection (an in-memory database, which only tests use).
    pub fn run_sync(&mut self) {
        if self.sync_busy {
            return;
        }
        let settings = settings_repo::get_all(&self.conn).unwrap_or_default();
        let Some(folder) = settings
            .get("sync_folder_path")
            .filter(|f| !f.trim().is_empty())
        else {
            self.sync_status = Some("Set a sync folder in Settings first".to_string());
            return;
        };
        // `Connection::path` returns `Some("")` — not `None` — for an
        // in-memory or temporary database, so an emptiness check is needed,
        // not just an `Option` one.
        let Some(db_path) = self.conn.path().filter(|p| !p.is_empty()) else {
            self.sync_status = Some("Sync isn't available for this database".to_string());
            return;
        };
        let device_id = self.sync_device_id();
        self.sync_pending = Some(sync::run_async(
            db_path.to_string(),
            folder.clone(),
            device_id,
        ));
        self.sync_busy = true;
        self.sync_status = Some("Syncing...".to_string());
    }

    /// Polled once per frame from `app.rs`, mirrors `poll_llm`. On
    /// completion, refreshes everything derived from the local database —
    /// the background thread merged into it through its own connection, so
    /// this thread's cached `visible_tasks`/`projects` won't reflect that
    /// on their own.
    pub fn poll_sync(&mut self) {
        let Some(rx) = &self.sync_pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(summary)) => {
                self.sync_pending = None;
                self.sync_busy = false;
                self.sync_status = Some(summary.describe());
                self.refresh_projects();
                self.refresh_visible_tasks();
            }
            Ok(Err(e)) => {
                self.sync_pending = None;
                self.sync_busy = false;
                self.sync_status = Some(format!("Sync failed: {e}"));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.sync_pending = None;
                self.sync_busy = false;
            }
        }
    }

    /// Called once per frame from `app.rs`, alongside `refresh_if_date_changed`
    /// — fires `run_sync` if a folder is configured and enough time has
    /// passed since the last attempt (or none has happened yet, which
    /// covers "sync on launch" as the very first tick). A longer interval
    /// than the date check, since this one does real file I/O against a
    /// possibly network-backed folder.
    pub fn maybe_auto_sync(&mut self) {
        const AUTO_SYNC_INTERVAL: Duration = Duration::minutes(15);
        if self.sync_busy {
            return;
        }
        let due = match self.last_auto_sync {
            None => true,
            Some(last) => Utc::now() - last >= AUTO_SYNC_INTERVAL,
        };
        if !due {
            return;
        }
        self.last_auto_sync = Some(Utc::now());
        self.run_sync();
    }

    /// Kicks off a calendar fetch on a background thread (see
    /// `calendar::run_async`) — a no-op if Google Calendar isn't connected,
    /// one's already in flight, or this database has no file path (an
    /// in-memory database, which only tests use). Mirrors `run_sync`.
    pub fn run_calendar_sync(&mut self) {
        if self.calendar_busy {
            return;
        }
        let Some(config) = self.google_calendar_config.clone() else {
            return;
        };
        let Some(db_path) = self.conn.path().filter(|p| !p.is_empty()) else {
            return;
        };
        let today = Local::now().date_naive();
        self.calendar_pending = Some(calendar::run_async(db_path.to_string(), config, today));
        self.calendar_busy = true;
    }

    /// Polled once per frame from `app.rs`, mirrors `poll_sync`.
    pub fn poll_calendar(&mut self) {
        let Some(rx) = &self.calendar_pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(events)) => {
                self.calendar_pending = None;
                self.calendar_busy = false;
                self.calendar_events = events;
                self.calendar_status = None;
                self.last_calendar_sync = Some(Utc::now());
            }
            Ok(Err(e)) => {
                self.calendar_pending = None;
                self.calendar_busy = false;
                self.calendar_status = Some(format!("Calendar sync failed: {e}"));
                self.last_calendar_sync = Some(Utc::now());
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.calendar_pending = None;
                self.calendar_busy = false;
            }
        }
    }

    /// Called once per frame from `app.rs`, mirrors `maybe_auto_sync` — but
    /// additionally gated on the Today perspective actually being shown, so
    /// an idle app sitting on some other view doesn't keep spending API
    /// quota fetching events nobody's looking at. Also called directly from
    /// `set_perspective` on switching to Today, so the first visit doesn't
    /// wait out the interval.
    pub fn maybe_auto_sync_calendar(&mut self) {
        if self.perspective != Perspective::Today || self.calendar_busy {
            return;
        }
        let due = match self.last_calendar_sync {
            None => true,
            Some(last) => Utc::now() - last >= self.calendar_refresh_interval,
        };
        if !due {
            return;
        }
        self.run_calendar_sync();
    }

    /// Starts the Google OAuth loopback flow (see `calendar::oauth`) using
    /// whatever client id/secret are currently typed into the open Settings
    /// draft. Persists them to `settings` immediately — not waiting for the
    /// screen's own Save button — so a background token refresh started
    /// later can find them even if the user closes Settings without hitting
    /// Save. A no-op if a connect attempt is already in flight or either
    /// field is blank.
    pub fn start_google_oauth(&mut self) {
        if self.google_auth_pending.is_some() {
            return;
        }
        let Some(draft) = &self.settings else {
            return;
        };
        let client_id = draft.gcal_client_id.trim().to_string();
        let client_secret = draft.gcal_client_secret.trim().to_string();
        if client_id.is_empty() || client_secret.is_empty() {
            self.google_auth_status = Some("Enter a client ID and client secret first".to_string());
            return;
        }
        if let Err(e) = settings_repo::set_many(
            &self.conn,
            &[
                ("gcal_client_id", client_id.as_str()),
                ("gcal_client_secret", client_secret.as_str()),
            ],
        ) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.google_auth_status =
            Some("Waiting for you to sign in with Google in your browser...".to_string());
        self.google_auth_pending = Some(calendar::connect_async(client_id, client_secret));
    }

    /// Polled once per frame from `app.rs`. On success, persists the new
    /// tokens, re-resolves `google_calendar_config` so the feature is live
    /// immediately, and kicks off an immediate fetch so Today's events show
    /// up without waiting for the next auto-sync tick.
    pub fn poll_google_auth(&mut self) {
        let Some(rx) = &self.google_auth_pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(tokens)) => {
                self.google_auth_pending = None;
                let expiry = tokens.expiry.to_rfc3339();
                let pairs = [
                    ("gcal_access_token", tokens.access_token.as_str()),
                    ("gcal_refresh_token", tokens.refresh_token.as_str()),
                    ("gcal_token_expiry", expiry.as_str()),
                ];
                if let Err(e) = settings_repo::set_many(&self.conn, &pairs) {
                    self.error_message = Some(e.to_string());
                    return;
                }
                self.google_auth_status = Some("Connected".to_string());
                if let Some(draft) = &mut self.settings {
                    draft.gcal_connected = true;
                }
                let settings = settings_repo::get_all(&self.conn).unwrap_or_default();
                self.google_calendar_config = GoogleCalendarConfig::resolve(&settings);
                self.run_calendar_sync();
            }
            Ok(Err(e)) => {
                self.google_auth_pending = None;
                self.google_auth_status = Some(format!("Connection failed: {e}"));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.google_auth_pending = None;
            }
        }
    }

    /// Clears the saved Google tokens (keeping the client id/secret, so
    /// reconnecting is one click) and stops showing events. Blanking rather
    /// than deleting the settings rows matches the existing "blank counts as
    /// absent" convention (see `GoogleCalendarConfig::resolve`) — there's no
    /// `settings_repo::delete` to reach for instead.
    pub fn disconnect_google_calendar(&mut self) {
        if let Err(e) = settings_repo::set_many(
            &self.conn,
            &[
                ("gcal_access_token", ""),
                ("gcal_refresh_token", ""),
                ("gcal_token_expiry", ""),
            ],
        ) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.google_calendar_config = None;
        self.calendar_events.clear();
        self.calendar_status = None;
        self.google_auth_status = None;
        if let Some(draft) = &mut self.settings {
            draft.gcal_connected = false;
        }
    }

    pub fn refresh_projects(&mut self) {
        let result = project_repo::list_all(&self.conn);
        self.projects = self.unwrap_or_report(result, Vec::new());
    }

    pub fn open_quick_capture(&mut self) {
        self.quick_capture_open = true;
        self.quick_capture_focus_pending = true;
    }

    pub fn close_quick_capture(&mut self) {
        self.quick_capture_open = false;
        self.quick_capture_focus_pending = false;
        self.quick_entry_buffer.clear();
        self.llm_pending = None;
        self.llm_busy = false;
    }

    pub fn quick_capture_submit(&mut self) {
        let title = self.quick_entry_buffer.trim();
        if title.is_empty() {
            return;
        }
        let mut task = Task::new_inbox(title);
        if let Perspective::Project(id) = self.perspective {
            task.project_id = Some(id);
        }
        if let Err(e) = task_repo::create(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.quick_entry_buffer.clear();
        self.quick_capture_open = false;
        self.refresh_visible_tasks();
    }

    pub fn llm_available(&self) -> bool {
        self.llm_config.is_some()
    }

    /// Enter in the quick-capture popup: sends the raw text to the configured
    /// LLM on a background thread to extract structured fields, instead of
    /// capturing it literally. Falls back to a plain `quick_capture_submit`
    /// if no provider is configured, so this is always safe to call. Shift+
    /// Enter bypasses this and calls `quick_capture_submit` directly, for
    /// when AI parsing would just get in the way.
    pub fn quick_capture_submit_with_ai(&mut self) {
        let Some(config) = self.llm_config.clone() else {
            self.quick_capture_submit();
            return;
        };
        let raw_text = self.quick_entry_buffer.trim().to_string();
        if raw_text.is_empty() {
            return;
        }
        let context = PromptContext {
            today: Local::now().date_naive(),
            project_names: self.projects.iter().map(|p| p.name.clone()).collect(),
        };
        self.llm_pending = Some(llm::parse_capture_async(config, raw_text, context));
        self.llm_busy = true;
    }

    /// Polled once per frame from `app.rs`. Non-blocking: does nothing while
    /// the background request is still running.
    pub fn poll_llm(&mut self) {
        let Some(rx) = &self.llm_pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(parsed)) => {
                self.llm_pending = None;
                self.llm_busy = false;
                self.apply_parsed_task(parsed);
            }
            Ok(Err(e)) => {
                self.llm_pending = None;
                self.llm_busy = false;
                self.error_message = Some(format!("AI capture failed, added as typed: {e}"));
                self.quick_capture_submit();
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.llm_pending = None;
                self.llm_busy = false;
            }
        }
    }

    /// Turns an LLM-parsed capture into a real task: matches `project` by
    /// name against known projects (never creates one — the prompt already
    /// tells the model not to invent names).
    fn apply_parsed_task(&mut self, parsed: ParsedTask) {
        let mut task = Task::new_inbox(parsed.title);
        // A recurring task with no explicit date still needs one to show up
        // anywhere (Today, the list's Due column, ...), so default it to
        // today — snapped forward off a disallowed weekday (e.g. "every
        // weekday" started on a Saturday) — rather than leaving it invisible
        // until edited by hand.
        task.due_date = parsed.due_date.or_else(|| {
            parsed
                .recurrence
                .map(|r| r.snap_to_allowed(Local::now().date_naive()))
        });
        task.recurrence = parsed.recurrence;

        if let Some(name) = parsed.project {
            task.project_id = self
                .projects
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(&name))
                .map(|p| p.id);
        } else if let Perspective::Project(id) = self.perspective {
            task.project_id = Some(id);
        }

        if let Err(e) = task_repo::create(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.quick_entry_buffer.clear();
        self.quick_capture_open = false;
        self.refresh_visible_tasks();
    }

    /// Sends the current chat input as a new user turn to the configured
    /// LLM, along with the full set of open tasks so it can resolve
    /// references like "the dentist task" and batch commands like "roll all
    /// of my overdue tasks to today". With no provider configured, appends
    /// an explanatory assistant message instead of starting a request.
    pub fn chat_send(&mut self) {
        let text = self.chat_input.trim().to_string();
        if text.is_empty() || self.chat_busy {
            return;
        }
        // Moving the conversation forward without confirming supersedes
        // whatever was proposed last turn — quietly drop it rather than
        // leaving it to be confirmed by a click that's about a different
        // reply by the time it lands.
        self.pending_chat_actions.clear();
        self.chat_input.clear();
        self.chat_history.push(ChatTurn {
            role: ChatRole::User,
            content: text,
        });

        let Some(config) = self.llm_config.clone() else {
            self.chat_history.push(ChatTurn {
                role: ChatRole::Assistant,
                content: "AI chat isn't configured — set WU_WEI_LLM_API_KEY (or a \
                          provider-specific key) to enable it."
                    .to_string(),
            });
            return;
        };

        let context = self.build_chat_context();
        self.chat_pending = Some(llm::send_chat_async(
            config,
            self.chat_history.clone(),
            context,
        ));
        self.chat_busy = true;
    }

    fn build_chat_context(&self) -> ChatContext {
        let project_name = |pid: crate::domain::project::ProjectId| {
            self.projects
                .iter()
                .find(|p| p.id == pid)
                .map(|p| p.name.clone())
        };
        let open_tasks = task_repo::list_open(&self.conn).unwrap_or_default();
        let tasks = open_tasks
            .iter()
            .map(|t| ChatTaskSummary {
                id: t.id,
                title: t.title.clone(),
                project: t.project_id.and_then(&project_name),
                due_date: t.due_date,
            })
            .collect();

        // "This week" for a summary request: the last 7 days, ending today.
        let today = Local::now().date_naive();
        let completed_since = today - Duration::days(6);
        let since_utc = completed_since
            .and_hms_opt(0, 0, 0)
            .and_then(|naive| naive.and_local_timezone(Local).earliest())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::days(7));
        let completed_recently = task_repo::list_completed_since(&self.conn, since_utc)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| {
                let completed_on = t.completed_at?.with_timezone(&Local).date_naive();
                Some(ChatCompletedTaskSummary {
                    id: t.id,
                    title: t.title.clone(),
                    project: t.project_id.and_then(&project_name),
                    completed_on,
                })
            })
            .collect();
        let calendar_events = self
            .calendar_events
            .iter()
            .map(|e| ChatCalendarEventSummary {
                title: e.title.clone(),
                time: if e.all_day {
                    "All day".to_string()
                } else {
                    e.start
                        .with_timezone(&Local)
                        .format("%-I:%M %p")
                        .to_string()
                },
                location: e.location.clone(),
            })
            .collect();
        ChatContext {
            today,
            tasks,
            project_names: self.projects.iter().map(|p| p.name.clone()).collect(),
            calendar_events,
            completed_since,
            completed_recently,
        }
    }

    /// Entry point for the AI panel's "Weekly summary" button: sends a fixed
    /// prompt as though the user had typed it, then restores whatever they
    /// had half-typed in the input box.
    pub fn request_weekly_summary(&mut self) {
        if self.chat_busy {
            return;
        }
        let half_typed = std::mem::take(&mut self.chat_input);
        self.chat_input = "Summarize what I accomplished this week from my completed tasks. \
             Group the work by project, keep it to a short readable recap, and skip routine \
             recurring chores."
            .to_string();
        self.chat_send();
        self.chat_input = half_typed;
    }

    /// Polled once per frame from `app.rs`. Non-blocking: does nothing while
    /// the background request is still running.
    pub fn poll_chat(&mut self) {
        let Some(rx) = &self.chat_pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(reply)) => {
                self.chat_pending = None;
                self.chat_busy = false;
                self.chat_focus_requested = true;
                self.receive_chat_reply(reply);
            }
            Ok(Err(e)) => {
                self.chat_pending = None;
                self.chat_busy = false;
                self.chat_focus_requested = true;
                self.chat_history.push(ChatTurn {
                    role: ChatRole::Assistant,
                    content: format!("Something went wrong: {e}"),
                });
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.chat_pending = None;
                self.chat_busy = false;
                self.chat_focus_requested = true;
            }
        }
    }

    /// Turns a model reply into a preview instead of acting on it: the
    /// model's prose, plus — when it proposed changes — a plain-language
    /// description of what those changes would be, appended to the
    /// transcript as one assistant turn. Nothing in `reply.actions` touches
    /// the database here. The model has repeatedly proposed changes for
    /// messages that were really just questions (e.g. "how about the whole
    /// weekend?", asked right after "what's on my list today?", once came
    /// back as a batch of unrelated recurrence changes), so every proposal
    /// waits in `pending_chat_actions` for `confirm_pending_chat_actions`
    /// (or `discard_pending_chat_actions`) regardless of how confident the
    /// model's own wording was.
    fn receive_chat_reply(&mut self, reply: ChatReply) {
        let mut failures = reply.parse_failures;
        let mut pending = Vec::new();
        // A project a `CreateProject` earlier in this same reply would make
        // — describing (never applying) each action in order still needs to
        // recognize it as valid for a later action in the same batch (e.g.
        // "create a Taxes project and move the W2 task into it"), even
        // though `self.projects` itself won't have it until confirmed.
        let mut projects_created_this_reply: Vec<String> = Vec::new();
        for action in reply.actions {
            match self.describe_chat_action(&action, &projects_created_this_reply) {
                Ok(description) => {
                    if let ChatAction::CreateProject { name } = &action {
                        projects_created_this_reply.push(name.clone());
                    }
                    pending.push(PendingChatAction {
                        action,
                        description,
                    });
                }
                Err(e) => failures.push(e),
            }
        }
        let failures = dedup_keep_order(failures);
        let mut content = reply.reply;
        if !pending.is_empty() {
            content.push_str("\n\nThis would: ");
            content.push_str(
                &pending
                    .iter()
                    .map(|p| p.description.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            );
            content.push_str(". Apply to confirm, or Discard to skip.");
        }
        if !failures.is_empty() {
            // The reply text above may claim something the model didn't
            // actually manage to propose correctly — say plainly that it
            // won't happen, rather than a bare parenthetical that reads as a
            // footnote.
            let prefix = if failures.len() == 1 {
                "This didn't actually happen: "
            } else {
                "These didn't actually happen: "
            };
            content.push_str("\n\n⚠ ");
            content.push_str(prefix);
            content.push_str(&failures.join("; "));
        }
        self.chat_history.push(ChatTurn {
            role: ChatRole::Assistant,
            content,
        });
        self.pending_chat_actions = pending;
    }

    /// Applies every currently pending action (see `pending_chat_actions`),
    /// appending a factual "Actions taken"/failure summary as a new
    /// assistant turn — independent of whatever the original reply's prose
    /// claimed. No-op if nothing is pending.
    pub fn confirm_pending_chat_actions(&mut self) {
        let pending = std::mem::take(&mut self.pending_chat_actions);
        if pending.is_empty() {
            return;
        }
        let mut done = Vec::new();
        let mut failures = Vec::new();
        for pending_action in pending {
            match self.apply_chat_action(pending_action.action) {
                Ok(description) => done.push(description),
                Err(e) => failures.push(e),
            }
        }
        let failures = dedup_keep_order(failures);
        let mut content = String::new();
        if !done.is_empty() {
            content.push_str("Actions taken: ");
            content.push_str(&done.join("; "));
            content.push('.');
        }
        if !failures.is_empty() {
            if !content.is_empty() {
                content.push_str("\n\n");
            }
            let prefix = if failures.len() == 1 {
                "This didn't actually happen: "
            } else {
                "These didn't actually happen: "
            };
            content.push_str("⚠ ");
            content.push_str(prefix);
            content.push_str(&failures.join("; "));
        }
        self.chat_history.push(ChatTurn {
            role: ChatRole::Assistant,
            content,
        });
        self.refresh_visible_tasks();
        self.refresh_projects();
    }

    /// Discards every currently pending action without applying any of
    /// them. No-op if nothing is pending.
    pub fn discard_pending_chat_actions(&mut self) {
        if self.pending_chat_actions.is_empty() {
            return;
        }
        self.pending_chat_actions.clear();
        self.chat_history.push(ChatTurn {
            role: ChatRole::Assistant,
            content: "Okay, I didn't make any changes.".to_string(),
        });
    }

    fn task_title(&self, task_id: TaskId) -> Result<String, String> {
        task_repo::get(&self.conn, task_id)
            .map_err(|e| e.to_string())?
            .map(|t| t.title)
            .ok_or_else(|| "no task with that id".to_string())
    }

    /// Number of tasks strictly overdue as of today — the working set a
    /// `RescheduleOverdue` action operates on.
    fn overdue_count(&self) -> Result<usize, String> {
        let today = Local::now().date_naive();
        task_repo::list_overdue(&self.conn, today)
            .map(|tasks| tasks.len())
            .map_err(|e| e.to_string())
    }

    /// Read-only counterpart to `apply_chat_action`, used to build the
    /// confirmation preview in `receive_chat_reply` before the user has
    /// agreed to anything. Resolves and validates the same targets it does
    /// (unknown task/project, a duplicate project name, ...) so a proposal
    /// that's actually going to fail is reported as a failure immediately
    /// rather than waiting for a confirm that was never going to work, but
    /// never performs the underlying database write.
    fn describe_chat_action(
        &self,
        action: &ChatAction,
        projects_created_this_reply: &[String],
    ) -> Result<String, String> {
        let project_exists = |name: &str| {
            self.projects
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(name))
                || projects_created_this_reply
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(name))
        };
        match action {
            ChatAction::SetDueDate { task_id, due_date } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("set the due date on \"{title}\" to {due_date}"))
            }
            ChatAction::RescheduleOverdue { due_date } => {
                let count = self.overdue_count()?;
                if count == 0 {
                    return Err("there are no overdue tasks to reschedule".to_string());
                }
                let noun = if count == 1 { "task" } else { "tasks" };
                Ok(format!("move {count} overdue {noun} to {due_date}"))
            }
            ChatAction::ClearDueDate { task_id } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("clear the due date on \"{title}\""))
            }
            ChatAction::SetRecurrence {
                task_id,
                recurrence,
            } => {
                let title = self.task_title(*task_id)?;
                Ok(format!(
                    "set \"{title}\" to repeat {}",
                    recurrence.describe()
                ))
            }
            ChatAction::ClearRecurrence { task_id } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("stop \"{title}\" from repeating"))
            }
            ChatAction::CompleteTask { task_id } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("complete \"{title}\""))
            }
            ChatAction::ReopenTask { task_id } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("reopen \"{title}\""))
            }
            ChatAction::MoveToProject { task_id, project } => {
                let title = self.task_title(*task_id)?;
                if !project_exists(project) {
                    return Err(format!("no project named {project:?}"));
                }
                Ok(format!("move \"{title}\" to {project}"))
            }
            ChatAction::MoveToInbox { task_id } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("move \"{title}\" to Inbox"))
            }
            ChatAction::CreateTask { task } => {
                let mut description = format!("create task \"{}\"", task.title);
                // Mirrors `apply_chat_action`'s CreateTask: an unknown
                // project name doesn't fail the action, it just files to
                // the Inbox, so the preview should describe the same
                // outcome rather than naming a project that won't be used.
                if let Some(project_name) =
                    task.project.as_ref().filter(|name| project_exists(name))
                {
                    description.push_str(&format!(" in {project_name}"));
                }
                if let Some(due) = task.due_date {
                    description.push_str(&format!(" due {due}"));
                }
                Ok(description)
            }
            ChatAction::CreateProject { name } => {
                if project_exists(name) {
                    return Err(format!("a project named {name:?} already exists"));
                }
                Ok(format!("create project \"{name}\""))
            }
            ChatAction::DeleteProject { project } => {
                if !project_exists(project) {
                    return Err(format!("no project named {project:?}"));
                }
                Ok(format!(
                    "delete project \"{project}\" (its tasks move to Inbox)"
                ))
            }
            ChatAction::DeleteTask { task_id } => {
                let title = self.task_title(*task_id)?;
                Ok(format!("permanently delete \"{title}\""))
            }
        }
    }

    /// Task deletion is deliberately excluded (see `ChatAction`'s doc
    /// comment for why project deletion isn't held to the same bar). On
    /// success, returns a short factual description for the "Actions taken"
    /// summary in `confirm_pending_chat_actions` — ground truth independent
    /// of the model's own reply text.
    fn apply_chat_action(&mut self, action: ChatAction) -> Result<String, String> {
        match action {
            ChatAction::SetDueDate { task_id, due_date } => {
                let title = self.task_title(task_id)?;
                task_repo::set_due_date(&self.conn, task_id, Some(due_date))
                    .map_err(|e| e.to_string())?;
                Ok(format!("set the due date on \"{title}\" to {due_date}"))
            }
            ChatAction::RescheduleOverdue { due_date } => {
                let today = Local::now().date_naive();
                let overdue =
                    task_repo::list_overdue(&self.conn, today).map_err(|e| e.to_string())?;
                if overdue.is_empty() {
                    return Err("there are no overdue tasks to reschedule".to_string());
                }
                let count = overdue.len();
                for task in &overdue {
                    task_repo::set_due_date(&self.conn, task.id, Some(due_date))
                        .map_err(|e| e.to_string())?;
                }
                let noun = if count == 1 { "task" } else { "tasks" };
                Ok(format!("moved {count} overdue {noun} to {due_date}"))
            }
            ChatAction::ClearDueDate { task_id } => {
                let title = self.task_title(task_id)?;
                task_repo::set_due_date(&self.conn, task_id, None).map_err(|e| e.to_string())?;
                Ok(format!("cleared the due date on \"{title}\""))
            }
            ChatAction::SetRecurrence {
                task_id,
                recurrence,
            } => {
                let title = self.task_title(task_id)?;
                task_repo::set_recurrence(&self.conn, task_id, Some(recurrence))
                    .map_err(|e| e.to_string())?;
                // Same rule as create_task/quick capture: a recurring task
                // with no due date still needs one to show up anywhere.
                let task = task_repo::get(&self.conn, task_id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "no task with that id".to_string())?;
                if task.due_date.is_none() {
                    let first = recurrence.snap_to_allowed(Local::now().date_naive());
                    task_repo::set_due_date(&self.conn, task_id, Some(first))
                        .map_err(|e| e.to_string())?;
                }
                Ok(format!(
                    "set \"{title}\" to repeat {}",
                    recurrence.describe()
                ))
            }
            ChatAction::ClearRecurrence { task_id } => {
                let title = self.task_title(task_id)?;
                task_repo::set_recurrence(&self.conn, task_id, None).map_err(|e| e.to_string())?;
                Ok(format!("stopped \"{title}\" from repeating"))
            }
            ChatAction::CompleteTask { task_id } => {
                let title = self.task_title(task_id)?;
                task_repo::set_completed(&self.conn, task_id, true).map_err(|e| e.to_string())?;
                self.spawn_next_occurrence(task_id);
                Ok(format!("completed \"{title}\""))
            }
            ChatAction::ReopenTask { task_id } => {
                let title = self.task_title(task_id)?;
                task_repo::set_completed(&self.conn, task_id, false).map_err(|e| e.to_string())?;
                Ok(format!("reopened \"{title}\""))
            }
            ChatAction::MoveToProject { task_id, project } => {
                let title = self.task_title(task_id)?;
                let project_id = self
                    .projects
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(&project))
                    .map(|p| p.id)
                    .ok_or_else(|| format!("no project named {project:?}"))?;
                task_repo::set_project(&self.conn, task_id, Some(project_id))
                    .map_err(|e| e.to_string())?;
                Ok(format!("moved \"{title}\" to {project}"))
            }
            ChatAction::MoveToInbox { task_id } => {
                let title = self.task_title(task_id)?;
                task_repo::set_project(&self.conn, task_id, None).map_err(|e| e.to_string())?;
                Ok(format!("moved \"{title}\" to Inbox"))
            }
            ChatAction::CreateTask { task: parsed } => {
                let mut task = Task::new_inbox(parsed.title.clone());
                // Same defaulting as quick capture: a recurring task with no
                // explicit date still needs one to show up anywhere, snapped
                // off a disallowed weekday.
                task.due_date = parsed.due_date.or_else(|| {
                    parsed
                        .recurrence
                        .map(|r| r.snap_to_allowed(Local::now().date_naive()))
                });
                task.recurrence = parsed.recurrence;
                if let Some(name) = &parsed.project {
                    task.project_id = self
                        .projects
                        .iter()
                        .find(|p| p.name.eq_ignore_ascii_case(name))
                        .map(|p| p.id);
                }
                task_repo::create(&self.conn, &task).map_err(|e| e.to_string())?;

                let mut description = format!("created task \"{}\"", task.title);
                if let Some(project_name) = task
                    .project_id
                    .and_then(|id| self.projects.iter().find(|p| p.id == id))
                    .map(|p| p.name.clone())
                {
                    description.push_str(&format!(" in {project_name}"));
                }
                if let Some(due) = task.due_date {
                    description.push_str(&format!(" due {due}"));
                }
                Ok(description)
            }
            ChatAction::CreateProject { name } => {
                if self
                    .projects
                    .iter()
                    .any(|p| p.name.eq_ignore_ascii_case(&name))
                {
                    return Err(format!("a project named {name:?} already exists"));
                }
                let project = Project::new(&name);
                project_repo::create(&self.conn, &project).map_err(|e| e.to_string())?;
                // Refreshed immediately (not just at the end of the batch)
                // so a later action in the same reply — e.g. "create a
                // Taxes project and move the W2 task into it" — can resolve
                // the new project by name.
                self.refresh_projects();
                Ok(format!("created project \"{name}\""))
            }
            ChatAction::DeleteProject { project } => {
                let project_id = self
                    .projects
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(&project))
                    .map(|p| p.id)
                    .ok_or_else(|| format!("no project named {project:?}"))?;
                self.delete_project_checked(project_id)?;
                Ok(format!("deleted project \"{project}\""))
            }
            ChatAction::DeleteTask { task_id } => {
                let title = self.task_title(task_id)?;
                self.delete_task_checked(task_id)?;
                Ok(format!("deleted \"{title}\""))
            }
        }
    }

    pub fn select_task(&mut self, id: TaskId) {
        self.selection = Selection::Task(id);
        self.highlighted_task = Some(id);
        self.project_edit_buffer = None;
        self.task_edit_buffer = self
            .visible_tasks
            .iter()
            .find(|t| t.id == id)
            .map(TaskEditBuffer::from_task);
        self.detail_panel_open = true;
    }

    pub fn select_project(&mut self, id: ProjectId) {
        self.selection = Selection::Project(id);
        self.task_edit_buffer = None;
        self.project_edit_buffer = self
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(ProjectEditBuffer::from_project);
        self.detail_panel_open = true;
    }

    /// Moves the keyboard cursor by `delta` rows within `visible_tasks` (negative
    /// moves up), without touching `selection` or the detail panel. With no
    /// current highlight, `Down` lands on the first task and `Up` on the last,
    /// matching common listbox conventions.
    pub fn move_highlight(&mut self, delta: i32) {
        if self.visible_tasks.is_empty() {
            return;
        }
        let current_index = self
            .highlighted_task
            .and_then(|id| self.visible_tasks.iter().position(|t| t.id == id));
        let next_index = match current_index {
            Some(i) => (i as i32 + delta).clamp(0, self.visible_tasks.len() as i32 - 1) as usize,
            None if delta >= 0 => 0,
            None => self.visible_tasks.len() - 1,
        };
        self.highlighted_task = Some(self.visible_tasks[next_index].id);
    }

    /// Enter's behavior on the highlighted row: opens its details if they
    /// aren't already showing, closes them if they are.
    pub fn toggle_highlighted_task_details(&mut self) {
        let Some(id) = self.highlighted_task else {
            return;
        };
        if self.selection == Selection::Task(id) {
            self.clear_selection();
        } else {
            self.select_task(id);
        }
    }

    /// Opens the keyboard-driven "move to project" picker for the highlighted
    /// task, pre-highlighting its current project (or Inbox).
    pub fn open_project_picker(&mut self) {
        let Some(task_id) = self.highlighted_task else {
            return;
        };
        let current_project = self
            .visible_tasks
            .iter()
            .find(|t| t.id == task_id)
            .and_then(|t| t.project_id);
        let highlighted = match current_project {
            None => 0,
            Some(id) => self
                .projects
                .iter()
                .position(|p| p.id == id)
                .map(|i| i + 1)
                .unwrap_or(0),
        };
        self.project_picker = Some(ProjectPickerState {
            task_id,
            highlighted,
        });
    }

    pub fn close_project_picker(&mut self) {
        self.project_picker = None;
    }

    pub fn move_project_picker_highlight(&mut self, delta: i32) {
        let Some(picker) = &mut self.project_picker else {
            return;
        };
        let max = self.projects.len() as i32;
        picker.highlighted = (picker.highlighted as i32 + delta).clamp(0, max) as usize;
    }

    /// Confirms the picker's currently highlighted row.
    pub fn confirm_project_picker(&mut self) {
        let Some(picker) = self.project_picker.take() else {
            return;
        };
        self.apply_picked_project(picker.task_id, picker.highlighted);
    }

    /// Confirms a specific row directly, e.g. on click (bypassing `highlighted`).
    pub fn pick_project_in_picker(&mut self, index: usize) {
        let Some(picker) = self.project_picker.take() else {
            return;
        };
        self.apply_picked_project(picker.task_id, index);
    }

    fn apply_picked_project(&mut self, task_id: TaskId, index: usize) {
        let project_id = if index == 0 {
            None
        } else {
            self.projects.get(index - 1).map(|p| p.id)
        };

        // If the task's details are open, go through the edit buffer so any
        // other unsaved field edits are preserved.
        if let Some(buf) = &mut self.task_edit_buffer
            && buf.id == task_id
        {
            buf.project_id = project_id;
            self.save_task_edits();
            return;
        }

        // Otherwise (details closed, task only highlighted) update it directly.
        let Some(mut task) = self.visible_tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        task.project_id = project_id;
        if let Err(e) = task_repo::update(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_visible_tasks();
    }

    /// Whether any keyboard-driven picker (project, due date, or estimate) is
    /// currently open, so callers can avoid opening a second one on top of it.
    pub fn any_picker_open(&self) -> bool {
        self.project_picker.is_some()
            || self.due_date_picker.is_some()
            || self.estimate_picker.is_some()
            || self.quick_capture_open
            || self.new_project_popup_open
            || self.settings.is_some()
            || self.archive_confirm_open
    }

    /// Opens the "Archive Completed" confirmation (see `ui::archive_confirm`)
    /// — the button that triggers this only appears in the Completed view.
    pub fn open_archive_confirm(&mut self) {
        self.archive_confirm_open = true;
    }

    pub fn close_archive_confirm(&mut self) {
        self.archive_confirm_open = false;
    }

    /// Permanently deletes every completed task — there is no undo. Clears
    /// the selection/highlight unconditionally first: the button that leads
    /// here only shows in the Completed view, where everything visible is
    /// about to be deleted anyway.
    pub fn confirm_archive_completed(&mut self) {
        self.archive_confirm_open = false;
        self.highlighted_task = None;
        self.clear_selection();
        if let Err(e) = task_repo::delete_completed(&self.conn) {
            self.error_message = Some(e.to_string());
        }
        self.refresh_visible_tasks();
    }

    /// Opens the Settings screen, seeding its draft from whatever's actually
    /// in effect right now (see `SettingsDraft::load`).
    pub fn open_settings(&mut self) {
        self.settings = Some(SettingsDraft::load(&self.conn, self.llm_config.as_ref()));
    }

    /// Closes Settings without saving — the draft (and anything typed into
    /// it) is simply dropped.
    pub fn close_settings(&mut self) {
        self.settings = None;
    }

    /// Opens the native "Browse…" dialog for the database file path, on a
    /// background thread (see `db_path_dialog`'s doc comment for why). A
    /// no-op if one's already open. `save_file`, not `pick_file`: the path
    /// may name a database that doesn't exist yet — Settings lets you point
    /// at a brand new file, not just choose among existing ones.
    pub fn browse_for_db_path(&mut self) {
        if self.db_path_dialog.is_some() {
            return;
        }
        let starting = self
            .settings
            .as_ref()
            .map(|d| d.db_path.clone())
            .unwrap_or_default();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let picked = crate::ui::settings::file_dialog_near(&starting)
                .add_filter("SQLite Database", &["db", "sqlite", "sqlite3"])
                .set_file_name("wu_wei.db")
                .save_file()
                .map(|p| p.display().to_string());
            let _ = tx.send(picked);
        });
        self.db_path_dialog = Some(rx);
    }

    /// Same shape as `browse_for_db_path`, for the sync folder field.
    pub fn browse_for_sync_folder(&mut self) {
        if self.sync_folder_dialog.is_some() {
            return;
        }
        let starting = self
            .settings
            .as_ref()
            .map(|d| d.sync_folder_path.clone())
            .unwrap_or_default();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let picked = crate::ui::settings::file_dialog_near(&starting)
                .pick_folder()
                .map(|p| p.display().to_string());
            let _ = tx.send(picked);
        });
        self.sync_folder_dialog = Some(rx);
    }

    /// Polled once per frame from `app.rs`. Non-blocking: does nothing while
    /// the dialog is still open. A cancelled dialog (`Ok(None)`) just clears
    /// the pending state, leaving whatever was already typed untouched.
    pub fn poll_db_path_dialog(&mut self) {
        let Some(rx) = &self.db_path_dialog else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(path)) => {
                self.db_path_dialog = None;
                if let Some(draft) = &mut self.settings {
                    draft.db_path = path;
                }
            }
            Ok(None) => self.db_path_dialog = None,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.db_path_dialog = None,
        }
    }

    /// Same shape as `poll_db_path_dialog`, for the sync folder field.
    pub fn poll_sync_folder_dialog(&mut self) {
        let Some(rx) = &self.sync_folder_dialog else {
            return;
        };
        match rx.try_recv() {
            Ok(Some(path)) => {
                self.sync_folder_dialog = None;
                if let Some(draft) = &mut self.settings {
                    draft.sync_folder_path = path;
                }
            }
            Ok(None) => self.sync_folder_dialog = None,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.sync_folder_dialog = None,
        }
    }

    /// Commits the Settings draft: persists the LLM fields to `settings`
    /// (reloading `llm_config` from the result, so AI features pick up the
    /// change immediately, no restart), and — if the db path changed —
    /// reconnects live via `switch_database` first, so the LLM fields end up
    /// saved in whichever database is active afterward.
    pub fn save_settings(&mut self) {
        let Some(draft) = self.settings.take() else {
            return;
        };

        let new_db_path = draft.db_path.trim().to_string();
        if !new_db_path.is_empty()
            && new_db_path != current_db_path(&self.conn)
            && let Err(e) = self.switch_database(&new_db_path)
        {
            self.error_message = Some(e);
        }

        let provider = match draft.llm_provider {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
        };
        let gcal_refresh_minutes = draft
            .gcal_refresh_minutes
            .clamp(calendar::MIN_REFRESH_MINUTES, calendar::MAX_REFRESH_MINUTES)
            .to_string();
        let pairs = [
            ("llm_provider", provider),
            ("llm_api_key", draft.llm_api_key.trim()),
            ("llm_base_url", draft.llm_base_url.trim()),
            ("llm_model", draft.llm_model.trim()),
            ("sync_folder_path", draft.sync_folder_path.trim()),
            ("gcal_client_id", draft.gcal_client_id.trim()),
            ("gcal_client_secret", draft.gcal_client_secret.trim()),
            ("gcal_refresh_minutes", gcal_refresh_minutes.as_str()),
        ];
        if let Err(e) = settings_repo::set_many(&self.conn, &pairs) {
            self.error_message = Some(e.to_string());
            return;
        }
        let settings = settings_repo::get_all(&self.conn).unwrap_or_default();
        self.llm_config = LlmConfig::resolve(&settings);
        self.calendar_refresh_interval = calendar_refresh_interval(&settings);
    }

    /// Reconnects to a different database file, live, and remembers it (see
    /// `db_bootstrap::remember_db_path`) so the next launch opens it too —
    /// that choice has to be made before any database is open, so it can't
    /// live inside one. Used when the db path changes in Settings.
    fn switch_database(&mut self, path: &str) -> Result<(), String> {
        let new_conn = db::open(path).map_err(|e| e.to_string())?;
        self.conn = new_conn;
        db_bootstrap::remember_db_path(path);
        self.perspective = Perspective::Inbox;
        self.clear_selection();
        self.highlighted_task = None;
        self.refresh_projects();
        self.refresh_visible_tasks();
        Ok(())
    }

    /// Opens the keyboard-driven due-date picker for the highlighted task,
    /// pre-highlighting whichever quick option matches its current due date
    /// (falling back to "No Due Date" if it doesn't match one exactly).
    pub fn open_due_date_picker(&mut self) {
        let Some(task_id) = self.highlighted_task else {
            return;
        };
        let current_due = self
            .visible_tasks
            .iter()
            .find(|t| t.id == task_id)
            .and_then(|t| t.due_date);
        let options = due_date_picker_options(Local::now().date_naive());
        let highlighted = options
            .iter()
            .position(|(_, date)| *date == current_due)
            .unwrap_or(0);
        self.due_date_picker = Some(DueDatePickerState {
            task_id,
            highlighted,
            text_input: String::new(),
            ai_pending: None,
            ai_error: None,
        });
    }

    pub fn close_due_date_picker(&mut self) {
        self.due_date_picker = None;
    }

    /// Whether the due-date picker is waiting on the LLM to resolve a typed
    /// phrase — the UI/shortcut layer freezes the option list while it is.
    pub fn due_date_picker_ai_busy(&self) -> bool {
        self.due_date_picker
            .as_ref()
            .is_some_and(|p| p.ai_pending.is_some())
    }

    pub fn move_due_date_picker_highlight(&mut self, delta: i32) {
        let Some(picker) = &mut self.due_date_picker else {
            return;
        };
        let max = due_date_picker_options(Local::now().date_naive()).len() as i32 - 1;
        picker.highlighted = (picker.highlighted as i32 + delta).clamp(0, max) as usize;
    }

    /// Enter in the picker's free-text field: resolves the typed phrase to a
    /// real date via the LLM on a background thread. A no-op when the field
    /// is blank; sets `ai_error` (rather than starting a request) when no LLM
    /// provider is configured.
    pub fn submit_due_date_picker_text(&mut self) {
        let Some(picker) = &mut self.due_date_picker else {
            return;
        };
        if picker.ai_pending.is_some() {
            return;
        }
        let text = picker.text_input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(config) = self.llm_config.clone() else {
            picker.ai_error = Some(
                "Typing a date needs an AI provider — set one up in Settings, or pick an \
                 option above."
                    .to_string(),
            );
            return;
        };
        picker.ai_error = None;
        picker.ai_pending = Some(llm::parse_due_date_async(
            config,
            text,
            Local::now().date_naive(),
        ));
    }

    /// Polled once per frame from `app.rs`. Non-blocking: does nothing while
    /// the background date-parse request is still running.
    pub fn poll_due_date_picker(&mut self) {
        let Some(picker) = &self.due_date_picker else {
            return;
        };
        let Some(rx) = &picker.ai_pending else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(date)) => {
                let task_id = picker.task_id;
                self.due_date_picker = None;
                self.set_task_due_date(task_id, Some(date));
            }
            Ok(Err(e)) => {
                if let Some(picker) = &mut self.due_date_picker {
                    picker.ai_pending = None;
                    picker.ai_error = Some(e);
                }
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                if let Some(picker) = &mut self.due_date_picker {
                    picker.ai_pending = None;
                }
            }
        }
    }

    /// Confirms the picker's currently highlighted row.
    pub fn confirm_due_date_picker(&mut self) {
        let Some(picker) = self.due_date_picker.take() else {
            return;
        };
        self.apply_picked_due_date(picker.task_id, picker.highlighted);
    }

    /// Confirms a specific row directly, e.g. on click (bypassing `highlighted`).
    pub fn pick_due_date_in_picker(&mut self, index: usize) {
        let Some(picker) = self.due_date_picker.take() else {
            return;
        };
        self.apply_picked_due_date(picker.task_id, index);
    }

    fn apply_picked_due_date(&mut self, task_id: TaskId, index: usize) {
        let options = due_date_picker_options(Local::now().date_naive());
        let Some((_, due_date)) = options.get(index) else {
            return;
        };
        self.set_task_due_date(task_id, *due_date);
    }

    /// Writes `due_date` (`None` clears it) onto `task_id`, whether it's
    /// currently open in the detail editor or only in the list — the shared
    /// tail of both the quick-option picker and its typed-date field.
    fn set_task_due_date(&mut self, task_id: TaskId, due_date: Option<NaiveDate>) {
        if let Some(buf) = &mut self.task_edit_buffer
            && buf.id == task_id
        {
            buf.due_date = due_date;
            self.save_task_edits();
            return;
        }

        let Some(mut task) = self.visible_tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        task.due_date = due_date;
        if let Err(e) = task_repo::update(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_visible_tasks();
    }

    /// Opens the keyboard-driven estimate picker for the highlighted task,
    /// pre-highlighting whichever quick option matches its current estimate
    /// (falling back to "No Estimate" if it doesn't match one exactly).
    pub fn open_estimate_picker(&mut self) {
        let Some(task_id) = self.highlighted_task else {
            return;
        };
        let current = self
            .visible_tasks
            .iter()
            .find(|t| t.id == task_id)
            .and_then(|t| t.estimated_minutes);
        let highlighted = estimate_picker_options()
            .iter()
            .position(|(_, minutes)| *minutes == current)
            .unwrap_or(0);
        self.estimate_picker = Some(EstimatePickerState {
            task_id,
            highlighted,
            text_input: String::new(),
            error: None,
        });
    }

    pub fn close_estimate_picker(&mut self) {
        self.estimate_picker = None;
    }

    pub fn move_estimate_picker_highlight(&mut self, delta: i32) {
        let Some(picker) = &mut self.estimate_picker else {
            return;
        };
        let max = estimate_picker_options().len() as i32 - 1;
        picker.highlighted = (picker.highlighted as i32 + delta).clamp(0, max) as usize;
    }

    /// Confirms the picker's currently highlighted row.
    pub fn confirm_estimate_picker(&mut self) {
        let Some(picker) = self.estimate_picker.take() else {
            return;
        };
        self.apply_picked_estimate(picker.task_id, picker.highlighted);
    }

    /// Confirms a specific row directly, e.g. on click (bypassing `highlighted`).
    pub fn pick_estimate_in_picker(&mut self, index: usize) {
        let Some(picker) = self.estimate_picker.take() else {
            return;
        };
        self.apply_picked_estimate(picker.task_id, index);
    }

    fn apply_picked_estimate(&mut self, task_id: TaskId, index: usize) {
        let Some((_, minutes)) = estimate_picker_options().get(index).cloned() else {
            return;
        };
        self.set_task_estimate(task_id, minutes);
    }

    /// Enter in the picker's free-text field: parses the typed phrase
    /// ("90", "1h30m", ...) locally into minutes and applies it. A no-op
    /// when the field is blank; sets an inline error when it can't be read.
    pub fn submit_estimate_picker_text(&mut self) {
        let Some(picker) = &mut self.estimate_picker else {
            return;
        };
        let text = picker.text_input.trim().to_string();
        if text.is_empty() {
            return;
        }
        match parse_estimate(&text) {
            Some(minutes) => {
                let task_id = picker.task_id;
                self.estimate_picker = None;
                self.set_task_estimate(task_id, Some(minutes));
            }
            None => {
                picker.error =
                    Some("Couldn't read an estimate from that — try \"90\", \"45m\", \"1h30m\".".to_string());
            }
        }
    }

    /// Writes `estimated_minutes` (`None` clears it) onto `task_id`, whether
    /// it's currently open in the detail editor or only in the list — the
    /// shared tail of both the quick-option picker and its typed field.
    fn set_task_estimate(&mut self, task_id: TaskId, minutes: Option<i64>) {
        if let Some(buf) = &mut self.task_edit_buffer
            && buf.id == task_id
        {
            buf.estimated_minutes = minutes;
            self.save_task_edits();
            return;
        }

        let Some(mut task) = self.visible_tasks.iter().find(|t| t.id == task_id).cloned() else {
            return;
        };
        task.estimated_minutes = minutes;
        if let Err(e) = task_repo::update(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_visible_tasks();
    }

    pub fn delete_task(&mut self, id: TaskId) {
        if let Err(e) = self.delete_task_checked(id) {
            self.error_message = Some(e);
        }
    }

    /// Core of `delete_task`, with the failure surfaced as a `Result`
    /// instead of routed straight to `self.error_message` — used by the AI
    /// chat panel, same as `delete_project_checked`.
    fn delete_task_checked(&mut self, id: TaskId) -> Result<(), String> {
        task_repo::delete(&self.conn, id).map_err(|e| e.to_string())?;
        if self.selection == Selection::Task(id) {
            self.clear_selection();
        }
        self.refresh_visible_tasks();
        Ok(())
    }

    pub fn delete_project(&mut self, id: ProjectId) {
        if let Err(e) = self.delete_project_checked(id) {
            self.error_message = Some(e);
        }
    }

    /// Core of `delete_project`, with the failure surfaced as a `Result`
    /// instead of routed straight to `self.error_message` — used by the AI
    /// chat panel, which needs to know whether the deletion it just
    /// described in its "actions taken" summary actually happened.
    fn delete_project_checked(&mut self, id: ProjectId) -> Result<(), String> {
        project_repo::delete(&self.conn, id).map_err(|e| e.to_string())?;
        self.refresh_projects();
        if self.perspective == Perspective::Project(id) {
            // The perspective we were viewing no longer exists; fall back to Inbox
            // (this also clears selection/edit buffers and refreshes the task list).
            self.set_perspective(Perspective::Inbox);
        } else {
            if self.selection == Selection::Project(id) {
                self.clear_selection();
            }
            // Tasks that belonged to this project moved to the Inbox (ON DELETE SET NULL),
            // which can change the current perspective's contents (e.g. viewing Inbox).
            self.refresh_visible_tasks();
        }
        Ok(())
    }

    pub fn save_task_edits(&mut self) {
        let Some(buf) = &self.task_edit_buffer else {
            return;
        };
        let task = Task {
            id: buf.id,
            title: buf.title.clone(),
            notes: buf.notes.clone(),
            project_id: buf.project_id,
            due_date: buf.due_date,
            defer_date: buf.defer_date,
            completed: buf.completed,
            completed_at: buf.completed_at,
            created_at: buf.created_at,
            // Ignored by `task_repo::update`, which always stamps its own
            // `Utc::now()` — no real value to put here.
            updated_at: buf.created_at,
            estimated_minutes: buf.estimated_minutes,
            recurrence: buf.recurrence,
        };
        if let Err(e) = task_repo::update(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_visible_tasks();
    }

    pub fn toggle_complete(&mut self, id: TaskId, completed: bool) {
        if let Err(e) = task_repo::set_completed(&self.conn, id, completed) {
            self.error_message = Some(e.to_string());
            return;
        }
        if completed {
            self.spawn_next_occurrence(id);
        }
        self.refresh_visible_tasks();
        if let Some(buf) = &self.task_edit_buffer
            && buf.id == id
        {
            self.select_task(id);
        }
    }

    /// If the just-completed task repeats, creates its next occurrence: a
    /// fresh task carrying over title/notes/project/recurrence, due on
    /// `recurrence.next_due_date` measured from today (the completion date —
    /// "repeat after completion" semantics). The completed task itself is
    /// left as-is, so it stays visible in Completed history.
    fn spawn_next_occurrence(&mut self, completed_id: TaskId) {
        let task = match task_repo::get(&self.conn, completed_id) {
            Ok(Some(t)) => t,
            Ok(None) => return,
            Err(e) => {
                self.error_message = Some(e.to_string());
                return;
            }
        };
        let Some(recurrence) = task.recurrence else {
            return;
        };
        let completed_on = task
            .completed_at
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| Local::now().date_naive());

        let mut next = Task::new_inbox(task.title);
        next.notes = task.notes;
        next.project_id = task.project_id;
        next.due_date = Some(recurrence.next_due_date(completed_on));
        next.estimated_minutes = task.estimated_minutes;
        next.recurrence = Some(recurrence);

        if let Err(e) = task_repo::create(&self.conn, &next) {
            self.error_message = Some(e.to_string());
        }
    }

    pub fn create_project(&mut self) {
        let name = self.new_project_name.trim();
        if name.is_empty() {
            return;
        }
        let project = Project::new(name);
        if let Err(e) = project_repo::create(&self.conn, &project) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.new_project_name.clear();
        self.refresh_projects();
    }

    pub fn open_new_project_popup(&mut self) {
        self.new_project_popup_open = true;
        self.new_project_focus_pending = true;
    }

    pub fn close_new_project_popup(&mut self) {
        self.new_project_popup_open = false;
        self.new_project_focus_pending = false;
        self.new_project_name.clear();
    }

    pub fn submit_new_project_popup(&mut self) {
        self.create_project();
        self.new_project_popup_open = false;
    }

    pub fn save_project_edits(&mut self) {
        let Some(buf) = &self.project_edit_buffer else {
            return;
        };
        let project = Project {
            id: buf.id,
            name: buf.name.clone(),
            notes: buf.notes.clone(),
            status: buf.status,
            kind: buf.kind,
            created_at: self
                .projects
                .iter()
                .find(|p| p.id == buf.id)
                .map(|p| p.created_at)
                .unwrap_or_else(Utc::now),
            // Ignored by `project_repo::update`, which always stamps its
            // own `Utc::now()` — no real value to put here.
            updated_at: Utc::now(),
        };
        if let Err(e) = project_repo::update(&self.conn, &project) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_projects();
    }

    fn unwrap_or_report<T>(&mut self, result: DbResult<T>, default: T) -> T {
        match result {
            Ok(v) => v,
            Err(e) => {
                self.error_message = Some(e.to_string());
                default
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_keep_order_drops_repeats_but_keeps_first_position() {
        let deduped = dedup_keep_order(
            ["a", "b", "a", "c", "b"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        );
        assert_eq!(deduped, vec!["a", "b", "c"]);
    }

    #[test]
    fn quick_capture_defaults_to_inbox() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "buy milk".to_string();
        state.quick_capture_submit();

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].title, "buy milk");
        assert!(state.visible_tasks[0].project_id.is_none());
    }

    #[test]
    fn overdue_perspective_shows_only_strictly_past_due_tasks() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let today = Local::now().date_naive();

        state.quick_entry_buffer = "overdue item".to_string();
        state.quick_capture_submit();
        let overdue_id = state.visible_tasks[0].id;
        state.select_task(overdue_id);
        state.task_edit_buffer.as_mut().unwrap().due_date = Some(today - Duration::days(1));
        state.save_task_edits();

        state.quick_entry_buffer = "due today item".to_string();
        state.quick_capture_submit();
        let today_id = state.visible_tasks.last().unwrap().id;
        state.select_task(today_id);
        state.task_edit_buffer.as_mut().unwrap().due_date = Some(today);
        state.save_task_edits();

        state.set_perspective(Perspective::Overdue);
        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].id, overdue_id);
    }

    #[test]
    fn review_perspective_shows_every_open_task_across_projects_and_inbox_but_not_completed() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        state.quick_entry_buffer = "inbox item".to_string();
        state.quick_capture_submit();

        state.set_perspective(Perspective::Project(project_id));
        state.quick_entry_buffer = "project item".to_string();
        state.quick_capture_submit();
        let project_item_id = state.visible_tasks[0].id;

        state.quick_entry_buffer = "finished project item".to_string();
        state.quick_capture_submit();
        let done_id = state.visible_tasks[1].id;
        state.toggle_complete(done_id, true);

        state.set_perspective(Perspective::Review);
        let ids: Vec<_> = state.visible_tasks.iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&project_item_id));
        assert!(!ids.contains(&done_id));
    }

    #[test]
    fn quick_capture_popup_opens_submits_and_closes() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        assert!(!state.quick_capture_open);

        state.open_quick_capture();
        assert!(state.quick_capture_open);
        assert!(state.any_picker_open());
        assert!(state.quick_capture_focus_pending);

        state.quick_entry_buffer = "call dentist".to_string();
        state.quick_capture_submit();

        assert!(!state.quick_capture_open);
        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].title, "call dentist");
    }

    #[test]
    fn closing_quick_capture_discards_the_draft() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.open_quick_capture();
        state.quick_entry_buffer = "half-typed".to_string();

        state.close_quick_capture();

        assert!(!state.quick_capture_open);
        assert!(!state.quick_capture_focus_pending);
        assert!(state.quick_entry_buffer.is_empty());
        assert!(state.visible_tasks.is_empty());
    }

    #[test]
    fn quick_capture_targets_current_project() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        state.set_perspective(Perspective::Project(project_id));
        state.quick_entry_buffer = "pick tile".to_string();
        state.quick_capture_submit();

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].project_id, Some(project_id));
    }

    #[test]
    fn apply_parsed_task_matches_project_case_insensitively_and_sets_due_date() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;
        let due = chrono::NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();

        state.apply_parsed_task(ParsedTask {
            title: "pick tile".to_string(),
            due_date: Some(due),
            project: Some("kitchen remodel".to_string()),
            recurrence: None,
        });

        // Perspective is still Inbox, so the project-filed task shouldn't show here.
        assert_eq!(state.visible_tasks.len(), 0);

        state.set_perspective(Perspective::Project(project_id));
        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].project_id, Some(project_id));
        assert_eq!(state.visible_tasks[0].due_date, Some(due));
    }

    #[test]
    fn apply_parsed_task_falls_back_to_current_project_perspective() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;
        state.set_perspective(Perspective::Project(project_id));

        state.apply_parsed_task(ParsedTask {
            title: "pick tile".to_string(),
            due_date: None,
            project: None,
            recurrence: None,
        });

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].project_id, Some(project_id));
    }

    #[test]
    fn apply_parsed_task_never_invents_an_unknown_project() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());

        state.apply_parsed_task(ParsedTask {
            title: "pick tile".to_string(),
            due_date: None,
            project: Some("Nonexistent Project".to_string()),
            recurrence: None,
        });

        assert!(state.projects.is_empty());
        assert_eq!(state.visible_tasks.len(), 1);
        assert!(state.visible_tasks[0].project_id.is_none());
    }

    #[test]
    fn apply_parsed_task_defaults_a_due_date_when_recurring_without_one() {
        use crate::domain::task::RecurrenceUnit;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.apply_parsed_task(ParsedTask {
            title: "inbox zero".to_string(),
            due_date: None,
            project: None,
            recurrence: Some(Recurrence::every(1, RecurrenceUnit::Days)),
        });

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(
            state.visible_tasks[0].due_date,
            Some(Local::now().date_naive())
        );
        assert_eq!(
            state.visible_tasks[0].recurrence,
            Some(Recurrence::every(1, RecurrenceUnit::Days))
        );
    }

    #[test]
    fn apply_parsed_task_snaps_an_every_weekday_default_off_the_weekend() {
        use crate::domain::task::{RecurrenceUnit, WeekdaySet};
        use chrono::Datelike;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.apply_parsed_task(ParsedTask {
            title: "daily standup".to_string(),
            due_date: None,
            project: None,
            recurrence: Some(
                Recurrence::every(1, RecurrenceUnit::Days)
                    .with_weekdays(Some(WeekdaySet::WEEKDAYS)),
            ),
        });

        let due = state.visible_tasks[0].due_date.unwrap();
        assert!(WeekdaySet::WEEKDAYS.contains(due.weekday()));
        let today = Local::now().date_naive();
        // On a weekday the default is still today; on a weekend it moved forward.
        if WeekdaySet::WEEKDAYS.contains(today.weekday()) {
            assert_eq!(due, today);
        } else {
            assert!(due > today);
        }
    }

    #[test]
    fn chat_action_set_due_date_updates_the_task_and_records_the_reply() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "overdue task".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        let today = Local::now().date_naive();

        state.receive_chat_reply(ChatReply {
            reply: "Rolled it to today.".to_string(),
            actions: vec![ChatAction::SetDueDate {
                task_id,
                due_date: today,
            }],
            parse_failures: Vec::new(),
        });

        assert_eq!(state.chat_history.len(), 1);
        assert_eq!(state.chat_history[0].role, ChatRole::Assistant);
        assert!(
            state.chat_history[0]
                .content
                .starts_with("Rolled it to today.")
        );
        assert!(
            state.chat_history[0]
                .content
                .contains("set the due date on \"overdue task\"")
        );
        assert_eq!(state.pending_chat_actions.len(), 1);

        // Nothing actually happens until the user confirms.
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(task.due_date, None);

        state.confirm_pending_chat_actions();

        assert!(state.pending_chat_actions.is_empty());
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(task.due_date, Some(today));
    }

    #[test]
    fn chat_action_reschedule_overdue_moves_every_overdue_task_at_once() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let today = Local::now().date_naive();

        for (title, due) in [
            ("way overdue", Some(today - Duration::days(10))),
            ("just overdue", Some(today - Duration::days(1))),
            ("due today", Some(today)),
            ("no due date", None),
        ] {
            let mut task = Task::new_inbox(title);
            task.due_date = due;
            task_repo::create(&state.conn, &task).unwrap();
        }

        state.receive_chat_reply(ChatReply {
            reply: "I'll roll your overdue tasks to today.".to_string(),
            actions: vec![ChatAction::RescheduleOverdue { due_date: today }],
            parse_failures: Vec::new(),
        });

        assert_eq!(state.pending_chat_actions.len(), 1);
        assert!(
            state.chat_history[0]
                .content
                .contains("move 2 overdue tasks to")
        );

        state.confirm_pending_chat_actions();

        let by_title = |t: &str| {
            task_repo::list_all(&state.conn)
                .unwrap()
                .into_iter()
                .find(|task| task.title == t)
                .unwrap()
        };
        assert_eq!(by_title("way overdue").due_date, Some(today));
        assert_eq!(by_title("just overdue").due_date, Some(today));
        assert_eq!(by_title("due today").due_date, Some(today));
        assert_eq!(by_title("no due date").due_date, None);
        assert!(
            state
                .chat_history
                .last()
                .unwrap()
                .content
                .contains("moved 2 overdue tasks to")
        );
    }

    #[test]
    fn chat_action_reschedule_overdue_with_nothing_overdue_is_reported_as_a_failure() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let today = Local::now().date_naive();

        state.receive_chat_reply(ChatReply {
            reply: "I'll roll your overdue tasks to today.".to_string(),
            actions: vec![ChatAction::RescheduleOverdue { due_date: today }],
            parse_failures: Vec::new(),
        });

        assert!(state.pending_chat_actions.is_empty());
        assert!(
            state.chat_history[0]
                .content
                .contains("no overdue tasks to reschedule")
        );
    }

    #[test]
    fn chat_action_move_to_project_resolves_by_name_case_insensitively() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        state.quick_entry_buffer = "pick tile".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.receive_chat_reply(ChatReply {
            reply: "Moved it.".to_string(),
            actions: vec![ChatAction::MoveToProject {
                task_id,
                project: "kitchen remodel".to_string(),
            }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(task.project_id, Some(project_id));
    }

    #[test]
    fn chat_action_unknown_project_reports_a_failure_without_crashing() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "pick tile".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.receive_chat_reply(ChatReply {
            reply: "Moved it.".to_string(),
            actions: vec![ChatAction::MoveToProject {
                task_id,
                project: "Nonexistent".to_string(),
            }],
            parse_failures: Vec::new(),
        });

        // Caught immediately (no task/project matched), no confirmation
        // needed for something that was never going to succeed.
        assert!(state.pending_chat_actions.is_empty());
        assert!(state.chat_history[0].content.contains("no project named"));
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert!(task.project_id.is_none());
    }

    #[test]
    fn chat_action_complete_task_spawns_next_occurrence_for_recurring_tasks() {
        use crate::domain::task::RecurrenceUnit;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "daily task".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.select_task(task_id);
        state.task_edit_buffer.as_mut().unwrap().recurrence =
            Some(Recurrence::every(1, RecurrenceUnit::Days));
        state.save_task_edits();

        state.receive_chat_reply(ChatReply {
            reply: "Done.".to_string(),
            actions: vec![ChatAction::CompleteTask { task_id }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        let all = task_repo::list_all(&state.conn).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn chat_action_delete_task_removes_it() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "obsolete task".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.receive_chat_reply(ChatReply {
            reply: "Deleted it.".to_string(),
            actions: vec![ChatAction::DeleteTask { task_id }],
            parse_failures: Vec::new(),
        });
        assert!(task_repo::get(&state.conn, task_id).unwrap().is_some());

        state.confirm_pending_chat_actions();

        assert!(task_repo::get(&state.conn, task_id).unwrap().is_none());
        assert!(
            state.chat_history[1]
                .content
                .contains("deleted \"obsolete task\"")
        );
    }

    #[test]
    fn chat_action_create_task_creates_a_recurring_task_in_a_project() {
        use crate::domain::task::RecurrenceUnit;
        use crate::llm::ParsedTask;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Habits".to_string();
        state.create_project();
        let project_id = state.projects[0].id;
        let due = chrono::NaiveDate::from_ymd_opt(2026, 8, 23).unwrap();

        state.receive_chat_reply(ChatReply {
            reply: "Added it.".to_string(),
            actions: vec![ChatAction::CreateTask {
                task: ParsedTask {
                    title: "Review Rocket Money".to_string(),
                    due_date: Some(due),
                    project: Some("Habits".to_string()),
                    recurrence: Some(Recurrence::every(1, RecurrenceUnit::Weeks)),
                },
            }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        state.set_perspective(Perspective::Project(project_id));
        assert_eq!(state.visible_tasks.len(), 1);
        let task = &state.visible_tasks[0];
        assert_eq!(task.title, "Review Rocket Money");
        assert_eq!(task.due_date, Some(due));
        assert_eq!(
            task.recurrence,
            Some(Recurrence::every(1, RecurrenceUnit::Weeks))
        );
        assert!(state.chat_history.iter().any(|t| {
            t.content
                .contains("created task \"Review Rocket Money\" in Habits")
        }));
    }

    #[test]
    fn chat_action_create_task_ignores_an_unknown_project_and_files_to_inbox() {
        use crate::llm::ParsedTask;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());

        state.receive_chat_reply(ChatReply {
            reply: "Added it.".to_string(),
            actions: vec![ChatAction::CreateTask {
                task: ParsedTask {
                    title: "buy milk".to_string(),
                    due_date: None,
                    project: Some("Nonexistent".to_string()),
                    recurrence: None,
                },
            }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        assert_eq!(state.visible_tasks.len(), 1);
        assert!(state.visible_tasks[0].project_id.is_none());
    }

    #[test]
    fn chat_action_set_recurrence_fixes_a_task_that_was_created_without_it() {
        use crate::domain::task::RecurrenceUnit;

        // Reproduces the reported scenario: a task exists (e.g. created
        // earlier without recurrence because the model left that optional
        // field empty) and the user asks the assistant to make it recur.
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "Deep Lunge".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        assert!(state.visible_tasks[0].recurrence.is_none());
        assert!(state.visible_tasks[0].due_date.is_none());

        state.receive_chat_reply(ChatReply {
            reply: "Done.".to_string(),
            actions: vec![ChatAction::SetRecurrence {
                task_id,
                recurrence: Recurrence::every(3, RecurrenceUnit::Days),
            }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(
            task.recurrence,
            Some(Recurrence::every(3, RecurrenceUnit::Days))
        );
        // A recurring task with no due date gets one so it actually shows up.
        assert_eq!(task.due_date, Some(Local::now().date_naive()));
        assert!(
            state.chat_history[0]
                .content
                .contains("set \"Deep Lunge\" to repeat every 3 days")
        );
    }

    #[test]
    fn chat_action_set_recurrence_keeps_an_existing_due_date() {
        use crate::domain::task::RecurrenceUnit;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "Deep Lunge".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        let due = Local::now().date_naive() + Duration::days(5);
        state.select_task(task_id);
        state.task_edit_buffer.as_mut().unwrap().due_date = Some(due);
        state.save_task_edits();

        state.receive_chat_reply(ChatReply {
            reply: "Done.".to_string(),
            actions: vec![ChatAction::SetRecurrence {
                task_id,
                recurrence: Recurrence::every(1, RecurrenceUnit::Weeks),
            }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(task.due_date, Some(due));
    }

    #[test]
    fn chat_action_clear_recurrence_stops_a_task_from_repeating_but_keeps_its_due_date() {
        use crate::domain::task::RecurrenceUnit;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "Deep Lunge".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.select_task(task_id);
        state.task_edit_buffer.as_mut().unwrap().recurrence =
            Some(Recurrence::every(1, RecurrenceUnit::Days));
        state.save_task_edits();
        let due = task_repo::get(&state.conn, task_id)
            .unwrap()
            .unwrap()
            .due_date;

        state.receive_chat_reply(ChatReply {
            reply: "Done.".to_string(),
            actions: vec![ChatAction::ClearRecurrence { task_id }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert!(task.recurrence.is_none());
        assert_eq!(task.due_date, due);
    }

    #[test]
    fn poll_chat_requests_focus_back_on_the_input_when_a_reply_arrives() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(ChatReply {
            reply: "Done.".to_string(),
            actions: Vec::new(),
            parse_failures: Vec::new(),
        }))
        .unwrap();
        state.chat_pending = Some(rx);
        state.chat_busy = true;

        state.poll_chat();

        assert!(!state.chat_busy);
        assert!(state.chat_focus_requested);
    }

    #[test]
    fn poll_chat_requests_focus_back_on_the_input_even_on_error() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err("network error".to_string())).unwrap();
        state.chat_pending = Some(rx);
        state.chat_busy = true;

        state.poll_chat();

        assert!(!state.chat_busy);
        assert!(state.chat_focus_requested);
    }

    #[test]
    fn repeated_identical_parse_failures_are_reported_once() {
        // Reproduces a real failure mode: the model mangled the same task
        // id the same way across several actions in one batch, producing
        // the identical "unparsable task_id" error five times over.
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let same_failure =
            "model returned an unparsable task_id \"bad\": invalid length".to_string();

        state.receive_chat_reply(ChatReply {
            reply: "Done.".to_string(),
            actions: Vec::new(),
            parse_failures: vec![same_failure.clone(); 5],
        });

        assert_eq!(
            state.chat_history[0].content,
            format!("Done.\n\n⚠ This didn't actually happen: {same_failure}")
        );
    }

    #[test]
    fn receive_chat_reply_stages_actions_without_touching_the_database() {
        // Reproduces a real failure mode: a follow-up question ("how about
        // the whole weekend?", after "what's on my list today?") came back
        // with a batch of unrelated recurrence changes on every visible
        // task. Whatever the model proposes, nothing should touch the
        // database until the user actually confirms it.
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "Garage WiFi".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.receive_chat_reply(ChatReply {
            reply: "Here's your weekend.".to_string(),
            actions: vec![ChatAction::SetRecurrence {
                task_id,
                recurrence: Recurrence::every(1, RecurrenceUnit::Days),
            }],
            parse_failures: Vec::new(),
        });

        assert_eq!(state.pending_chat_actions.len(), 1);
        assert!(
            state.chat_history[0]
                .content
                .contains("set \"Garage WiFi\" to repeat every day")
        );
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert!(task.recurrence.is_none());
    }

    #[test]
    fn discard_pending_chat_actions_drops_them_without_applying() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "Garage WiFi".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.receive_chat_reply(ChatReply {
            reply: "Here's your weekend.".to_string(),
            actions: vec![ChatAction::CompleteTask { task_id }],
            parse_failures: Vec::new(),
        });
        assert_eq!(state.pending_chat_actions.len(), 1);

        state.discard_pending_chat_actions();

        assert!(state.pending_chat_actions.is_empty());
        assert_eq!(state.chat_history.len(), 2);
        assert_eq!(
            state.chat_history[1].content,
            "Okay, I didn't make any changes."
        );
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert!(!task.completed);
    }

    #[test]
    fn discard_pending_chat_actions_is_a_noop_with_nothing_pending() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.discard_pending_chat_actions();
        assert!(state.chat_history.is_empty());
    }

    #[test]
    fn chat_context_includes_this_weeks_non_recurring_completed_tasks() {
        let state = AppState::new(crate::db::open_in_memory().unwrap());

        let mut done = Task::new_inbox("shipped the thing");
        done.completed = true;
        done.completed_at = Some(Utc::now() - Duration::days(2));
        task_repo::create(&state.conn, &done).unwrap();

        let mut old = Task::new_inbox("finished last month");
        old.completed = true;
        old.completed_at = Some(Utc::now() - Duration::days(30));
        task_repo::create(&state.conn, &old).unwrap();

        let mut chore = Task::new_inbox("water the plants");
        chore.completed = true;
        chore.completed_at = Some(Utc::now() - Duration::days(1));
        chore.recurrence = Some(Recurrence::every(1, RecurrenceUnit::Weeks));
        task_repo::create(&state.conn, &chore).unwrap();

        let ctx = state.build_chat_context();
        let titles: Vec<&str> = ctx
            .completed_recently
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, vec!["shipped the thing"]);
        assert_eq!(
            ctx.completed_since,
            Local::now().date_naive() - Duration::days(6)
        );
    }

    #[test]
    fn request_weekly_summary_sends_a_prompt_and_preserves_the_draft() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.llm_config = None; // chat_send's early-return path is enough here
        state.chat_input = "half-typed question".to_string();

        state.request_weekly_summary();

        assert_eq!(state.chat_input, "half-typed question");
        assert_eq!(state.chat_history[0].role, ChatRole::User);
        assert!(
            state.chat_history[0]
                .content
                .contains("accomplished this week")
        );
    }

    #[test]
    fn sending_a_new_message_drops_any_unconfirmed_pending_actions() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "Garage WiFi".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.llm_config = None; // chat_send's early-return path is enough here

        state.receive_chat_reply(ChatReply {
            reply: "Here's your weekend.".to_string(),
            actions: vec![ChatAction::CompleteTask { task_id }],
            parse_failures: Vec::new(),
        });
        assert_eq!(state.pending_chat_actions.len(), 1);

        state.chat_input = "never mind".to_string();
        state.chat_send();

        assert!(state.pending_chat_actions.is_empty());
    }

    #[test]
    fn chat_send_without_config_appends_an_explanatory_message() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.llm_config = None;
        state.chat_input = "roll overdue tasks to today".to_string();
        state.chat_send();

        assert_eq!(state.chat_history.len(), 2);
        assert_eq!(state.chat_history[0].role, ChatRole::User);
        assert_eq!(state.chat_history[1].role, ChatRole::Assistant);
        assert!(state.chat_history[1].content.contains("isn't configured"));
        assert!(state.chat_input.is_empty());
        assert!(state.chat_pending.is_none());
    }

    #[test]
    fn chat_action_create_project_makes_it_immediately_available_to_later_actions() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "file W2".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.receive_chat_reply(ChatReply {
            reply: "Created Taxes and filed it there.".to_string(),
            actions: vec![
                ChatAction::CreateProject {
                    name: "Taxes".to_string(),
                },
                ChatAction::MoveToProject {
                    task_id,
                    project: "Taxes".to_string(),
                },
            ],
            parse_failures: Vec::new(),
        });
        // Both actions described cleanly — MoveToProject resolves "Taxes"
        // against the CreateProject earlier in the same batch even though
        // it doesn't exist in `self.projects` yet.
        assert_eq!(state.pending_chat_actions.len(), 2);
        state.confirm_pending_chat_actions();

        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.projects[0].name, "Taxes");
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(task.project_id, Some(state.projects[0].id));
    }

    #[test]
    fn chat_action_create_project_refuses_a_duplicate_name() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Taxes".to_string();
        state.create_project();

        state.receive_chat_reply(ChatReply {
            reply: "Done.".to_string(),
            actions: vec![ChatAction::CreateProject {
                name: "taxes".to_string(),
            }],
            parse_failures: Vec::new(),
        });

        // Caught immediately, no confirmation needed for something that
        // was never going to succeed.
        assert!(state.pending_chat_actions.is_empty());
        assert_eq!(state.projects.len(), 1);
        assert!(state.chat_history[0].content.contains("already exists"));
    }

    #[test]
    fn chat_action_delete_project_unfiles_its_tasks_instead_of_deleting_them() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Taxes".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        state.set_perspective(Perspective::Project(project_id));
        state.quick_entry_buffer = "file W2".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.set_perspective(Perspective::Inbox);

        state.receive_chat_reply(ChatReply {
            reply: "Deleted Taxes.".to_string(),
            actions: vec![ChatAction::DeleteProject {
                project: "Taxes".to_string(),
            }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        assert!(state.projects.is_empty());
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert!(task.project_id.is_none());
    }

    #[test]
    fn actions_taken_summary_reflects_only_what_was_actually_attempted() {
        // Reproduces a real failure mode: the model's reply text narrates a
        // project deletion it never actually included an action for — only
        // an unrelated, real action (completing a task) was sent. There's
        // nothing to fail here (no malformed or missing-field action), so
        // the only way the user can tell the deletion never happened is the
        // factual "Actions taken" summary, independent of the reply text.
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Recurring".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        state.set_perspective(Perspective::Project(project_id));
        state.quick_entry_buffer = "Read".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.set_perspective(Perspective::Inbox);

        state.receive_chat_reply(ChatReply {
            reply: "Deleted project 'Recurring' and moved its tasks to Inbox.".to_string(),
            actions: vec![ChatAction::CompleteTask { task_id }],
            parse_failures: Vec::new(),
        });
        state.confirm_pending_chat_actions();

        assert_eq!(state.projects.len(), 1);
        assert_eq!(state.projects[0].name, "Recurring");
        assert!(
            state.chat_history[1]
                .content
                .contains("Actions taken: completed \"Read\".")
        );
    }

    #[test]
    fn a_real_deletion_failure_is_reported_instead_of_claimed_as_success() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Taxes".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        // Delete the project out from under the chat action so
        // `project_repo::delete` genuinely fails (0 rows affected is fine
        // for DELETE, so force a real error via a closed-out id instead:
        // simplest is to already not exist in the DB while still being in
        // the in-memory cache the chat handler resolves names against).
        project_repo::delete(&state.conn, project_id).unwrap();

        state.receive_chat_reply(ChatReply {
            reply: "Deleted Taxes.".to_string(),
            actions: vec![ChatAction::DeleteProject {
                project: "Taxes".to_string(),
            }],
            parse_failures: Vec::new(),
        });
        // The stale in-memory cache still has "Taxes", so the preview
        // describes it fine — the real failure only surfaces once confirmed
        // and the actual database delete is attempted.
        assert_eq!(state.pending_chat_actions.len(), 1);
        state.confirm_pending_chat_actions();

        assert!(
            !state.chat_history[1]
                .content
                .contains("Actions taken: deleted")
        );
        assert!(
            state.chat_history[1]
                .content
                .contains("This didn't actually happen")
        );
    }

    #[test]
    fn a_malformed_action_is_noted_without_dropping_the_reply_or_other_actions() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "overdue task".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        let today = Local::now().date_naive();

        // Mirrors what a malformed model reply looks like once parsed: one
        // good action plus a note about the one the model got wrong,
        // instead of the whole turn being replaced by a bare error.
        state.receive_chat_reply(ChatReply {
            reply: "Rolled it to today.".to_string(),
            actions: vec![ChatAction::SetDueDate {
                task_id,
                due_date: today,
            }],
            parse_failures: vec![
                "model's \"create_project\" action is missing a project".to_string(),
            ],
        });

        assert_eq!(state.chat_history.len(), 1);
        assert!(
            state.chat_history[0]
                .content
                .starts_with("Rolled it to today.")
        );
        assert!(state.chat_history[0].content.contains("missing a project"));
        assert_eq!(state.pending_chat_actions.len(), 1);

        state.confirm_pending_chat_actions();
        let task = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(task.due_date, Some(today));
    }

    #[test]
    fn move_highlight_does_not_open_details() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "first".to_string();
        state.quick_capture_submit();
        state.quick_entry_buffer = "second".to_string();
        state.quick_capture_submit();

        state.move_highlight(1);
        assert_eq!(state.highlighted_task, Some(state.visible_tasks[0].id));
        assert_eq!(state.selection, Selection::None);
        assert!(state.task_edit_buffer.is_none());

        state.move_highlight(1);
        assert_eq!(state.highlighted_task, Some(state.visible_tasks[1].id));
        assert_eq!(state.selection, Selection::None);

        // Clamps at the end instead of wrapping.
        state.move_highlight(1);
        assert_eq!(state.highlighted_task, Some(state.visible_tasks[1].id));
    }

    #[test]
    fn toggle_highlighted_task_details_opens_then_closes() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "task".to_string();
        state.quick_capture_submit();
        let id = state.visible_tasks[0].id;

        state.move_highlight(1);
        assert_eq!(state.selection, Selection::None);

        state.toggle_highlighted_task_details();
        assert_eq!(state.selection, Selection::Task(id));
        assert!(state.task_edit_buffer.is_some());

        state.toggle_highlighted_task_details();
        assert_eq!(state.selection, Selection::None);
        assert!(state.task_edit_buffer.is_none());
        // Toggling closed does not forget the keyboard cursor position.
        assert_eq!(state.highlighted_task, Some(id));
    }

    #[test]
    fn project_picker_moves_highlighted_task_without_opening_details() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        state.quick_entry_buffer = "pick tile".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.move_highlight(1);
        assert_eq!(state.selection, Selection::None);

        state.open_project_picker();
        assert!(state.project_picker.is_some());
        state.move_project_picker_highlight(1); // 0 = Inbox, 1 = first project
        state.confirm_project_picker();

        assert!(state.project_picker.is_none());
        state.refresh_visible_tasks(); // task left the Inbox perspective
        assert!(!state.visible_tasks.iter().any(|t| t.id == task_id));

        let moved = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(moved.project_id, Some(project_id));
    }

    #[test]
    fn this_weekend_returns_same_day_if_already_weekend() {
        let saturday = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        assert_eq!(saturday.weekday(), Weekday::Sat);
        assert_eq!(this_weekend(saturday), saturday);

        let sunday = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
        assert_eq!(sunday.weekday(), Weekday::Sun);
        assert_eq!(this_weekend(sunday), sunday);
    }

    #[test]
    fn this_weekend_finds_upcoming_saturday() {
        let monday = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        assert_eq!(monday.weekday(), Weekday::Mon);
        assert_eq!(
            this_weekend(monday),
            NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()
        );
    }

    #[test]
    fn next_week_is_always_a_strictly_future_monday() {
        let monday = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        assert_eq!(
            next_week(monday),
            NaiveDate::from_ymd_opt(2026, 8, 24).unwrap()
        );

        let wednesday = NaiveDate::from_ymd_opt(2026, 8, 19).unwrap();
        assert_eq!(
            next_week(wednesday),
            NaiveDate::from_ymd_opt(2026, 8, 24).unwrap()
        );
    }

    #[test]
    fn due_date_picker_options_lists_no_date_first() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        let options = due_date_picker_options(today);
        assert_eq!(options.len(), 5);
        assert_eq!(options[0], ("No Due Date".to_string(), None));
        assert_eq!(options[1].1, Some(today));
    }

    #[test]
    fn due_date_picker_sets_date_on_highlighted_task_without_opening_details() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.move_highlight(1);
        assert_eq!(state.selection, Selection::None);

        state.open_due_date_picker();
        assert!(state.due_date_picker.is_some());
        state.move_due_date_picker_highlight(2); // 0 = None, 1 = Today, 2 = Tomorrow
        state.confirm_due_date_picker();

        assert!(state.due_date_picker.is_none());
        let expected = Local::now().date_naive() + Duration::days(1);
        let updated = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(updated.due_date, Some(expected));
    }

    #[test]
    fn estimate_picker_options_lists_no_estimate_first() {
        let options = estimate_picker_options();
        assert_eq!(options[0], ("No Estimate".to_string(), None));
        assert_eq!(options[1].1, Some(15));
    }

    #[test]
    fn parse_estimate_reads_the_common_forms() {
        assert_eq!(parse_estimate("90"), Some(90));
        assert_eq!(parse_estimate("45m"), Some(45));
        assert_eq!(parse_estimate("45 min"), Some(45));
        assert_eq!(parse_estimate("2h"), Some(120));
        assert_eq!(parse_estimate("2 hours"), Some(120));
        assert_eq!(parse_estimate("1h30m"), Some(90));
        assert_eq!(parse_estimate("1h 30"), Some(90));
        assert_eq!(parse_estimate("1.5h"), Some(90));
        assert_eq!(parse_estimate(""), None);
        assert_eq!(parse_estimate("0"), None);
        assert_eq!(parse_estimate("soon"), None);
    }

    #[test]
    fn estimate_picker_sets_minutes_on_highlighted_task_without_opening_details() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "draft proposal".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.move_highlight(1);
        assert_eq!(state.selection, Selection::None);

        state.open_estimate_picker();
        assert!(state.estimate_picker.is_some());
        state.move_estimate_picker_highlight(4); // 0 = None … 4 = "1 hour"
        state.confirm_estimate_picker();

        assert!(state.estimate_picker.is_none());
        let updated = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(updated.estimated_minutes, Some(60));
    }

    #[test]
    fn estimate_picker_text_field_parses_a_typed_value() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "draft proposal".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.move_highlight(1);
        state.open_estimate_picker();

        state.estimate_picker.as_mut().unwrap().text_input = "1h30m".to_string();
        state.submit_estimate_picker_text();

        assert!(state.estimate_picker.is_none());
        let updated = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(updated.estimated_minutes, Some(90));
    }

    #[test]
    fn estimate_picker_text_field_reports_an_unreadable_value() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "draft proposal".to_string();
        state.quick_capture_submit();
        state.move_highlight(1);
        state.open_estimate_picker();

        state.estimate_picker.as_mut().unwrap().text_input = "whenever".to_string();
        state.submit_estimate_picker_text();

        let picker = state.estimate_picker.as_ref().unwrap();
        assert!(picker.error.is_some());
    }

    #[test]
    fn submit_due_date_picker_text_without_a_provider_sets_an_inline_error() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.llm_config = None;
        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        state.move_highlight(1);
        state.open_due_date_picker();

        state.due_date_picker.as_mut().unwrap().text_input = "next friday".to_string();
        state.submit_due_date_picker_text();

        let picker = state.due_date_picker.as_ref().unwrap();
        assert!(picker.ai_pending.is_none());
        assert!(picker.ai_error.as_deref().unwrap().contains("AI provider"));
    }

    #[test]
    fn submit_due_date_picker_text_is_a_noop_when_the_field_is_blank() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        state.move_highlight(1);
        state.open_due_date_picker();

        state.due_date_picker.as_mut().unwrap().text_input = "   ".to_string();
        state.submit_due_date_picker_text();

        let picker = state.due_date_picker.as_ref().unwrap();
        assert!(picker.ai_pending.is_none());
        assert!(picker.ai_error.is_none());
    }

    #[test]
    fn poll_due_date_picker_applies_a_resolved_date_and_closes_the_picker() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.move_highlight(1);
        state.open_due_date_picker();

        // Stand in for the background LLM thread: hand the picker a channel
        // that already carries a resolved date.
        let (tx, rx) = std::sync::mpsc::channel();
        let resolved = Local::now().date_naive() + Duration::days(9);
        tx.send(Ok(resolved)).unwrap();
        state.due_date_picker.as_mut().unwrap().ai_pending = Some(rx);

        state.poll_due_date_picker();

        assert!(state.due_date_picker.is_none());
        let updated = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(updated.due_date, Some(resolved));
    }

    #[test]
    fn poll_due_date_picker_keeps_the_picker_open_and_shows_a_parse_failure() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        state.move_highlight(1);
        state.open_due_date_picker();

        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Err("couldn't read a date from that".to_string()))
            .unwrap();
        state.due_date_picker.as_mut().unwrap().ai_pending = Some(rx);

        state.poll_due_date_picker();

        let picker = state.due_date_picker.as_ref().unwrap();
        assert!(picker.ai_pending.is_none());
        assert_eq!(
            picker.ai_error.as_deref(),
            Some("couldn't read a date from that")
        );
    }

    #[test]
    fn sidebar_entries_lists_projects_in_render_order() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        let project_id = state.projects[0].id;

        let entries = state.sidebar_entries();
        assert_eq!(
            entries,
            vec![
                Perspective::Inbox,
                Perspective::Today,
                Perspective::Overdue,
                Perspective::Review,
                Perspective::Completed,
                Perspective::Project(project_id),
            ]
        );
    }

    #[test]
    fn move_sidebar_highlight_switches_perspective_and_stays_focused() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.focus_sidebar(0); // Inbox
        assert_eq!(state.perspective, Perspective::Inbox);
        assert!(state.sidebar_focused);

        state.move_sidebar_highlight(1);
        assert_eq!(state.perspective, Perspective::Today);
        assert!(state.sidebar_focused);

        state.move_sidebar_highlight(-1);
        assert_eq!(state.perspective, Perspective::Inbox);

        // Clamps at the ends instead of wrapping.
        state.move_sidebar_highlight(-1);
        assert_eq!(state.perspective, Perspective::Inbox);
    }

    #[test]
    fn focusing_a_sidebar_row_immediately_highlights_its_first_task() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "water plants".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.focus_sidebar(0); // Inbox
        assert!(state.sidebar_focused);
        // Navigating to the row places the keyboard inside its content right
        // away, with no separate step required to "enter" the list.
        assert_eq!(state.highlighted_task, Some(task_id));
        // But it doesn't go as far as opening the detail panel.
        assert_eq!(state.selection, Selection::None);
    }

    #[test]
    fn moving_the_sidebar_highlight_keeps_the_first_task_synced() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "water plants".to_string();
        state.quick_capture_submit();
        let inbox_task = state.visible_tasks[0].id;

        state.quick_entry_buffer = "renew passport".to_string();
        state.quick_capture_submit();
        let completed_task = state.visible_tasks[1].id;
        state.toggle_complete(completed_task, true);

        state.focus_sidebar(0); // Inbox
        assert_eq!(state.highlighted_task, Some(inbox_task));

        state.move_sidebar_highlight(1); // Today (empty)
        assert!(state.highlighted_task.is_none());

        state.move_sidebar_highlight(1); // Overdue (empty)
        assert!(state.highlighted_task.is_none());

        state.move_sidebar_highlight(1); // Review (only the still-open task)
        assert_eq!(state.perspective, Perspective::Review);
        assert_eq!(state.highlighted_task, Some(inbox_task));

        state.move_sidebar_highlight(1); // Completed
        assert_eq!(state.perspective, Perspective::Completed);
        assert_eq!(state.highlighted_task, Some(completed_task));
    }

    #[test]
    fn exit_sidebar_focus_only_releases_arrow_keys() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "water plants".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.focus_sidebar(0); // Inbox
        assert_eq!(state.highlighted_task, Some(task_id));

        state.exit_sidebar_focus();
        assert!(!state.sidebar_focused);
        // The already-synced highlight carries over unchanged.
        assert_eq!(state.highlighted_task, Some(task_id));
        assert_eq!(state.selection, Selection::None);
    }

    #[test]
    fn detail_panel_opens_on_selection_and_closes_when_cleared() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "water plants".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        assert!(!state.detail_panel_open);

        state.select_task(task_id);
        assert!(state.detail_panel_open);

        // Switching perspective clears the selection, and the panel with it.
        state.set_perspective(Perspective::Today);
        assert!(!state.detail_panel_open);

        state.set_perspective(Perspective::Inbox);
        state.select_task(task_id);
        assert!(state.detail_panel_open);

        // Toggling details closed via Enter also closes the panel.
        state.highlighted_task = Some(task_id);
        state.toggle_highlighted_task_details();
        assert_eq!(state.selection, Selection::None);
        assert!(!state.detail_panel_open);
    }

    #[test]
    fn detail_panel_toggle_button_works_regardless_of_selection() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        assert!(!state.detail_panel_open);
        assert_eq!(state.selection, Selection::None);

        // The info button can force it open with nothing selected.
        state.toggle_detail_panel();
        assert!(state.detail_panel_open);

        state.toggle_detail_panel();
        assert!(!state.detail_panel_open);
    }

    #[test]
    fn deleting_the_selected_task_closes_the_detail_panel() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "water plants".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.select_task(task_id);
        assert!(state.detail_panel_open);

        state.delete_task(task_id);
        assert!(!state.detail_panel_open);
    }

    #[test]
    fn completing_a_recurring_task_spawns_its_next_occurrence() {
        use crate::domain::task::RecurrenceUnit;

        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "water plants".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.select_task(task_id);
        state.task_edit_buffer.as_mut().unwrap().recurrence =
            Some(Recurrence::every(3, RecurrenceUnit::Days));
        state.save_task_edits();

        state.toggle_complete(task_id, true);

        let all = task_repo::list_all(&state.conn).unwrap();
        assert_eq!(all.len(), 2);

        let original = all.iter().find(|t| t.id == task_id).unwrap();
        assert!(original.completed);

        let next = all.iter().find(|t| t.id != task_id).unwrap();
        assert!(!next.completed);
        assert_eq!(next.title, "water plants");
        assert_eq!(
            next.recurrence,
            Some(Recurrence::every(3, RecurrenceUnit::Days))
        );
        let expected_due = original.completed_at.unwrap().date_naive() + chrono::Duration::days(3);
        assert_eq!(next.due_date, Some(expected_due));
    }

    #[test]
    fn completing_a_non_recurring_task_spawns_nothing() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "one-off".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;

        state.toggle_complete(task_id, true);

        let all = task_repo::list_all(&state.conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn open_settings_seeds_the_draft_from_the_active_llm_config() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.llm_config = Some(LlmConfig {
            provider: ProviderKind::Anthropic,
            api_key: "sk-test".to_string(),
            base_url: "https://example.test".to_string(),
            model: "claude-test".to_string(),
        });

        state.open_settings();

        let draft = state.settings.as_ref().unwrap();
        assert_eq!(draft.llm_provider, ProviderKind::Anthropic);
        assert_eq!(draft.llm_api_key, "sk-test");
        assert_eq!(draft.llm_base_url, "https://example.test");
        assert_eq!(draft.llm_model, "claude-test");
    }

    #[test]
    fn close_settings_discards_the_draft() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.open_settings();
        assert!(state.settings.is_some());

        state.close_settings();

        assert!(state.settings.is_none());
    }

    #[test]
    fn save_settings_persists_llm_fields_and_updates_the_live_config() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.open_settings();
        {
            let draft = state.settings.as_mut().unwrap();
            draft.llm_provider = ProviderKind::Anthropic;
            draft.llm_api_key = "sk-live".to_string();
            draft.llm_base_url = "https://example.test".to_string();
            draft.llm_model = "claude-test".to_string();
        }

        state.save_settings();

        assert!(state.settings.is_none());
        let config = state.llm_config.as_ref().unwrap();
        assert_eq!(config.provider, ProviderKind::Anthropic);
        assert_eq!(config.api_key, "sk-live");
        assert_eq!(config.base_url, "https://example.test");
        assert_eq!(config.model, "claude-test");

        let saved = settings_repo::get_all(&state.conn).unwrap();
        assert_eq!(
            saved.get("llm_api_key").map(String::as_str),
            Some("sk-live")
        );
    }

    #[test]
    fn save_settings_persists_the_calendar_refresh_rate_and_updates_the_live_interval() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        assert_eq!(
            state.calendar_refresh_interval,
            Duration::minutes(calendar::DEFAULT_REFRESH_MINUTES as i64)
        );

        state.open_settings();
        state.settings.as_mut().unwrap().gcal_refresh_minutes = 20;
        state.save_settings();

        assert_eq!(state.calendar_refresh_interval, Duration::minutes(20));
        let saved = settings_repo::get_all(&state.conn).unwrap();
        assert_eq!(
            saved.get("gcal_refresh_minutes").map(String::as_str),
            Some("20")
        );

        // A reload of the draft reflects the saved value.
        state.open_settings();
        assert_eq!(state.settings.as_ref().unwrap().gcal_refresh_minutes, 20);
    }

    #[test]
    fn save_settings_clamps_an_out_of_range_calendar_refresh_rate() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.open_settings();
        state.settings.as_mut().unwrap().gcal_refresh_minutes = 10_000_000;
        state.save_settings();

        assert_eq!(
            state.calendar_refresh_interval,
            Duration::minutes(calendar::MAX_REFRESH_MINUTES as i64)
        );
    }

    #[test]
    fn blank_saved_llm_fields_dont_shadow_the_live_env_derived_config() {
        // Regression test: a blank value already sitting in `settings` (e.g.
        // Settings was once saved before the env-derived config had loaded)
        // must not permanently mask a real, working `.env`-sourced value —
        // it should be treated the same as no saved value at all.
        let conn = crate::db::open_in_memory().unwrap();
        settings_repo::set_many(
            &conn,
            &[
                ("llm_provider", ""),
                ("llm_api_key", ""),
                ("llm_base_url", ""),
                ("llm_model", ""),
            ],
        )
        .unwrap();

        let live_config = LlmConfig {
            provider: ProviderKind::Anthropic,
            api_key: "sk-from-env".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            model: "claude-from-env".to_string(),
        };

        let draft = SettingsDraft::load(&conn, Some(&live_config));
        assert_eq!(draft.llm_provider, ProviderKind::Anthropic);
        assert_eq!(draft.llm_api_key, "sk-from-env");
        assert_eq!(draft.llm_base_url, "https://api.anthropic.com");
        assert_eq!(draft.llm_model, "claude-from-env");
    }

    #[test]
    fn save_settings_leaves_the_database_untouched_when_the_path_field_is_unchanged() {
        // Regression test: `SettingsDraft::load` and `save_settings` must
        // agree on what "the current db path" is, or a Save with the path
        // field left completely alone looks like a change and triggers a
        // pointless (and, on a real db, disruptive) reconnect.
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "before save".to_string();
        state.quick_capture_submit();

        state.open_settings();
        state.save_settings();

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].title, "before save");
    }

    #[test]
    fn save_settings_reconnects_when_the_db_path_field_changes() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "in the old database".to_string();
        state.quick_capture_submit();
        assert_eq!(state.visible_tasks.len(), 1);

        state.open_settings();
        state.settings.as_mut().unwrap().db_path = ":memory:".to_string();
        state.save_settings();

        // A fresh `:memory:` database has nothing in it, and the
        // perspective/selection reset along with the reconnect.
        assert_eq!(state.visible_tasks.len(), 0);
        assert_eq!(state.perspective, Perspective::Inbox);
        assert_eq!(state.selection, Selection::None);
    }

    #[test]
    fn confirm_archive_completed_permanently_deletes_completed_tasks_only() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "keep me".to_string();
        state.quick_capture_submit();
        state.quick_entry_buffer = "archive me".to_string();
        state.quick_capture_submit();
        let to_complete = state
            .visible_tasks
            .iter()
            .find(|t| t.title == "archive me")
            .unwrap()
            .id;
        state.toggle_complete(to_complete, true);

        state.set_perspective(Perspective::Completed);
        assert_eq!(state.visible_tasks.len(), 1);

        state.open_archive_confirm();
        assert!(state.archive_confirm_open);
        state.confirm_archive_completed();

        assert!(!state.archive_confirm_open);
        assert_eq!(state.visible_tasks.len(), 0);

        let all = task_repo::list_all(&state.conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].title, "keep me");
    }

    #[test]
    fn close_archive_confirm_leaves_completed_tasks_in_place() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "done thing".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.toggle_complete(task_id, true);
        state.set_perspective(Perspective::Completed);

        state.open_archive_confirm();
        state.close_archive_confirm();

        assert!(!state.archive_confirm_open);
        assert_eq!(state.visible_tasks.len(), 1);
        let all = task_repo::list_all(&state.conn).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn confirm_archive_completed_closes_the_detail_panel_for_a_deleted_task() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "done thing".to_string();
        state.quick_capture_submit();
        let task_id = state.visible_tasks[0].id;
        state.toggle_complete(task_id, true);
        state.set_perspective(Perspective::Completed);
        state.select_task(task_id);
        assert_eq!(state.selection, Selection::Task(task_id));

        state.confirm_archive_completed();

        assert_eq!(state.selection, Selection::None);
        assert!(!state.detail_panel_open);
    }

    #[test]
    fn refresh_if_date_changed_requeries_once_the_date_has_moved_on() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        // Inserted directly via the repo, bypassing quick_capture (which
        // refreshes on its own) — `visible_tasks` won't see it until
        // something actually re-queries.
        task_repo::create(&state.conn, &Task::new_inbox("added while asleep")).unwrap();
        assert!(state.visible_tasks.is_empty());

        // A date from the past always differs from the real "today",
        // regardless of when the test runs.
        state.last_seen_date = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
        state.refresh_if_date_changed();

        assert_eq!(state.last_seen_date, Local::now().date_naive());
        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].title, "added while asleep");
    }

    #[test]
    fn refresh_if_date_changed_is_a_noop_when_the_date_is_unchanged() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        task_repo::create(&state.conn, &Task::new_inbox("added later, same day")).unwrap();
        assert!(state.visible_tasks.is_empty());

        state.refresh_if_date_changed();

        assert!(state.visible_tasks.is_empty());
    }

    #[test]
    fn run_sync_without_a_configured_folder_sets_a_status_and_does_nothing() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());

        state.run_sync();

        assert!(!state.sync_busy);
        assert!(state.sync_pending.is_none());
        assert!(state.sync_status.is_some());
    }

    #[test]
    fn run_sync_bails_out_for_an_in_memory_database() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        settings_repo::set(&state.conn, "sync_folder_path", "/tmp/wherever").unwrap();

        state.run_sync();

        assert!(!state.sync_busy);
        assert!(state.sync_pending.is_none());
        let status = state.sync_status.as_deref().unwrap_or_default();
        assert!(
            status.contains("isn't available"),
            "unexpected status: {status}"
        );
    }

    #[test]
    fn set_sort_by_due_date_defaults_to_descending_with_no_due_date_last() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let today = Local::now().date_naive();

        state.quick_entry_buffer = "no due date".to_string();
        state.quick_capture_submit();

        state.quick_entry_buffer = "due soon".to_string();
        state.quick_capture_submit();
        let due_soon_id = state
            .visible_tasks
            .iter()
            .find(|t| t.title == "due soon")
            .unwrap()
            .id;
        state.select_task(due_soon_id);
        state.task_edit_buffer.as_mut().unwrap().due_date = Some(today + Duration::days(1));
        state.save_task_edits();

        state.quick_entry_buffer = "due later".to_string();
        state.quick_capture_submit();
        let due_later_id = state
            .visible_tasks
            .iter()
            .find(|t| t.title == "due later")
            .unwrap()
            .id;
        state.select_task(due_later_id);
        state.task_edit_buffer.as_mut().unwrap().due_date = Some(today + Duration::days(5));
        state.save_task_edits();

        state.set_sort(TaskSortKey::DueDate);
        assert_eq!(state.sort_direction, SortDirection::Descending);

        let titles: Vec<&str> = state
            .visible_tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["due later", "due soon", "no due date"]);

        // Clicking the same header again flips to ascending, still with the
        // undated task last.
        state.set_sort(TaskSortKey::DueDate);
        assert_eq!(state.sort_direction, SortDirection::Ascending);
        let titles: Vec<&str> = state
            .visible_tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["due soon", "due later", "no due date"]);
    }

    #[test]
    fn set_sort_by_estimate_defaults_to_ascending_with_no_estimate_last() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());

        state.quick_entry_buffer = "no estimate".to_string();
        state.quick_capture_submit();

        state.quick_entry_buffer = "quick".to_string();
        state.quick_capture_submit();
        let quick_id = state
            .visible_tasks
            .iter()
            .find(|t| t.title == "quick")
            .unwrap()
            .id;
        state.select_task(quick_id);
        state.task_edit_buffer.as_mut().unwrap().estimated_minutes = Some(15);
        state.save_task_edits();

        state.quick_entry_buffer = "long".to_string();
        state.quick_capture_submit();
        let long_id = state
            .visible_tasks
            .iter()
            .find(|t| t.title == "long")
            .unwrap()
            .id;
        state.select_task(long_id);
        state.task_edit_buffer.as_mut().unwrap().estimated_minutes = Some(240);
        state.save_task_edits();

        state.set_sort(TaskSortKey::Estimate);
        assert_eq!(state.sort_direction, SortDirection::Ascending);
        let titles: Vec<&str> = state.visible_tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["quick", "long", "no estimate"]);

        // Flip to descending, undated task still last.
        state.set_sort(TaskSortKey::Estimate);
        assert_eq!(state.sort_direction, SortDirection::Descending);
        let titles: Vec<&str> = state.visible_tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["long", "quick", "no estimate"]);
    }

    #[test]
    fn set_sort_by_project_orders_alphabetically_and_toggles_direction() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());

        state.new_project_name = "Zzz Project".to_string();
        state.create_project();
        let zzz_id = state
            .projects
            .iter()
            .find(|p| p.name == "Zzz Project")
            .unwrap()
            .id;

        state.new_project_name = "Aaa Project".to_string();
        state.create_project();
        let aaa_id = state
            .projects
            .iter()
            .find(|p| p.name == "Aaa Project")
            .unwrap()
            .id;

        state.set_perspective(Perspective::Project(zzz_id));
        state.quick_entry_buffer = "zzz task".to_string();
        state.quick_capture_submit();

        state.set_perspective(Perspective::Project(aaa_id));
        state.quick_entry_buffer = "aaa task".to_string();
        state.quick_capture_submit();

        state.set_perspective(Perspective::Review);
        state.quick_entry_buffer = "inbox task".to_string();
        state.quick_capture_submit();

        state.set_sort(TaskSortKey::Project);
        assert_eq!(state.sort_direction, SortDirection::Ascending);
        let titles: Vec<&str> = state
            .visible_tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["aaa task", "inbox task", "zzz task"]);

        state.set_sort(TaskSortKey::Project);
        assert_eq!(state.sort_direction, SortDirection::Descending);
        let titles: Vec<&str> = state
            .visible_tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["zzz task", "inbox task", "aaa task"]);
    }

    #[test]
    fn switching_perspective_preserves_the_chosen_sort() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        let today = Local::now().date_naive();

        state.quick_entry_buffer = "no due date".to_string();
        state.quick_capture_submit();
        state.quick_entry_buffer = "due today".to_string();
        state.quick_capture_submit();
        let due_today_id = state
            .visible_tasks
            .iter()
            .find(|t| t.title == "due today")
            .unwrap()
            .id;
        state.select_task(due_today_id);
        state.task_edit_buffer.as_mut().unwrap().due_date = Some(today);
        state.save_task_edits();

        state.set_sort(TaskSortKey::DueDate);
        state.set_perspective(Perspective::Review);

        let titles: Vec<&str> = state
            .visible_tasks
            .iter()
            .map(|t| t.title.as_str())
            .collect();
        assert_eq!(titles, ["due today", "no due date"]);
    }
}
