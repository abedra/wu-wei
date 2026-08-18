use chrono::{Datelike, Duration, NaiveDate, Utc, Weekday};
use rusqlite::Connection;

use crate::db::error::DbResult;
use crate::db::{project_repo, tag_repo, task_repo};
use crate::domain::project::{Project, ProjectId, ProjectKind, ProjectStatus};
use crate::domain::tag::{Tag, TagId};
use crate::domain::task::{Task, TaskId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perspective {
    Inbox,
    Today,
    Flagged,
    Completed,
    AllProjects,
    Project(ProjectId),
    AllTags,
    Tag(TagId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    None,
    Task(TaskId),
    Project(ProjectId),
    Tag(TagId),
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
    pub flagged: bool,
    pub completed: bool,
    pub completed_at: Option<chrono::DateTime<Utc>>,
    pub created_at: chrono::DateTime<Utc>,
    pub estimated_minutes: Option<i64>,
    pub tag_names: Vec<String>,
    pub new_tag_input: String,
}

impl TaskEditBuffer {
    fn from_task(task: &Task, all_tags: &[Tag]) -> Self {
        let tag_names = task
            .tags
            .iter()
            .filter_map(|id| {
                all_tags
                    .iter()
                    .find(|t| t.id == *id)
                    .map(|t| t.name.clone())
            })
            .collect();
        TaskEditBuffer {
            id: task.id,
            title: task.title.clone(),
            notes: task.notes.clone(),
            project_id: task.project_id,
            due_date: task.due_date,
            defer_date: task.defer_date,
            flagged: task.flagged,
            completed: task.completed,
            completed_at: task.completed_at,
            created_at: task.created_at,
            estimated_minutes: task.estimated_minutes,
            tag_names,
            new_tag_input: String::new(),
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

pub struct TagEditBuffer {
    pub id: TagId,
    pub name: String,
}

impl TagEditBuffer {
    fn from_tag(tag: &Tag) -> Self {
        TagEditBuffer {
            id: tag.id,
            name: tag.name.clone(),
        }
    }
}

pub struct AppState {
    pub conn: Connection,
    pub projects: Vec<Project>,
    pub tags: Vec<Tag>,
    pub visible_tasks: Vec<Task>,
    pub perspective: Perspective,
    pub selection: Selection,
    /// Keyboard cursor over `visible_tasks`, moved by the Up/Down arrows.
    /// Independent of `selection`: moving it does not open the detail panel
    /// (mirrors OmniFocus, where arrow keys move the row cursor without
    /// forcing the inspector open). `Enter` toggles the detail panel for
    /// whichever task this points at; Space/flag/delete/move-to-project all
    /// act on it too, whether or not the detail panel happens to be open.
    pub highlighted_task: Option<TaskId>,
    pub task_edit_buffer: Option<TaskEditBuffer>,
    pub project_edit_buffer: Option<ProjectEditBuffer>,
    pub tag_edit_buffer: Option<TagEditBuffer>,
    pub quick_entry_buffer: String,
    /// Whether the quick-capture popup is open (toggled by Cmd+N). It's a
    /// floating window rather than a permanent panel, so it never sits in the
    /// normal Tab order when closed.
    pub quick_capture_open: bool,
    pub new_project_name: String,
    pub new_tag_name: String,
    pub project_picker: Option<ProjectPickerState>,
    pub due_date_picker: Option<DueDatePickerState>,
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
    pub error_message: Option<String>,
}

impl AppState {
    pub fn new(conn: Connection) -> Self {
        let mut state = AppState {
            conn,
            projects: Vec::new(),
            tags: Vec::new(),
            visible_tasks: Vec::new(),
            perspective: Perspective::Inbox,
            selection: Selection::None,
            highlighted_task: None,
            task_edit_buffer: None,
            project_edit_buffer: None,
            tag_edit_buffer: None,
            quick_entry_buffer: String::new(),
            quick_capture_open: false,
            new_project_name: String::new(),
            new_tag_name: String::new(),
            project_picker: None,
            due_date_picker: None,
            sidebar_focused: false,
            detail_panel_open: false,
            error_message: None,
        };
        state.refresh_projects();
        state.refresh_tags();
        state.refresh_visible_tasks();
        state
    }

    pub fn set_perspective(&mut self, p: Perspective) {
        self.perspective = p;
        self.clear_selection();
        self.highlighted_task = None;
        self.sidebar_focused = false;
        self.refresh_visible_tasks();
    }

    /// Clears whatever is selected and its edit buffer, closing the detail
    /// panel along with it (see `AppState::detail_panel_open`).
    fn clear_selection(&mut self) {
        self.selection = Selection::None;
        self.task_edit_buffer = None;
        self.project_edit_buffer = None;
        self.tag_edit_buffer = None;
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
            Perspective::Flagged,
            Perspective::Completed,
            Perspective::AllProjects,
        ];
        entries.extend(self.projects.iter().map(|p| Perspective::Project(p.id)));
        entries.push(Perspective::AllTags);
        entries.extend(self.tags.iter().map(|t| Perspective::Tag(t.id)));
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
        let today = Utc::now().date_naive();
        let result = match self.perspective {
            Perspective::Inbox => task_repo::list_inbox(&self.conn),
            Perspective::Today => task_repo::list_today(&self.conn, today),
            Perspective::Flagged => task_repo::list_flagged(&self.conn),
            Perspective::Completed => task_repo::list_completed(&self.conn),
            Perspective::AllProjects => Ok(Vec::new()),
            Perspective::Project(id) => task_repo::list_by_project(&self.conn, id),
            Perspective::AllTags => Ok(Vec::new()),
            Perspective::Tag(id) => task_repo::list_by_tag(&self.conn, id),
        };
        self.visible_tasks = self.unwrap_or_report(result, Vec::new());
        if let Some(id) = self.highlighted_task
            && !self.visible_tasks.iter().any(|t| t.id == id)
        {
            self.highlighted_task = None;
        }
    }

    pub fn refresh_projects(&mut self) {
        let result = project_repo::list_all(&self.conn);
        self.projects = self.unwrap_or_report(result, Vec::new());
    }

    pub fn refresh_tags(&mut self) {
        let result = tag_repo::list_all(&self.conn);
        self.tags = self.unwrap_or_report(result, Vec::new());
    }

    pub fn open_quick_capture(&mut self) {
        self.quick_capture_open = true;
    }

    pub fn close_quick_capture(&mut self) {
        self.quick_capture_open = false;
        self.quick_entry_buffer.clear();
    }

    pub fn quick_capture_submit(&mut self) {
        let title = self.quick_entry_buffer.trim();
        if title.is_empty() {
            return;
        }
        let mut task = Task::new_inbox(title);
        match self.perspective {
            Perspective::Project(id) => task.project_id = Some(id),
            Perspective::Tag(id) => task.tags = vec![id],
            _ => {}
        }
        if let Err(e) = task_repo::create(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.quick_entry_buffer.clear();
        self.quick_capture_open = false;
        self.refresh_visible_tasks();
    }

    pub fn select_task(&mut self, id: TaskId) {
        self.selection = Selection::Task(id);
        self.highlighted_task = Some(id);
        self.project_edit_buffer = None;
        self.tag_edit_buffer = None;
        self.task_edit_buffer = self
            .visible_tasks
            .iter()
            .find(|t| t.id == id)
            .map(|t| TaskEditBuffer::from_task(t, &self.tags));
        self.detail_panel_open = true;
    }

    pub fn select_project(&mut self, id: ProjectId) {
        self.selection = Selection::Project(id);
        self.task_edit_buffer = None;
        self.tag_edit_buffer = None;
        self.project_edit_buffer = self
            .projects
            .iter()
            .find(|p| p.id == id)
            .map(ProjectEditBuffer::from_project);
        self.detail_panel_open = true;
    }

    pub fn select_tag(&mut self, id: TagId) {
        self.selection = Selection::Tag(id);
        self.task_edit_buffer = None;
        self.project_edit_buffer = None;
        self.tag_edit_buffer = self
            .tags
            .iter()
            .find(|t| t.id == id)
            .map(TagEditBuffer::from_tag);
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

    /// Whether any keyboard-driven picker (project or due date) is currently
    /// open, so callers can avoid opening a second one on top of it.
    pub fn any_picker_open(&self) -> bool {
        self.project_picker.is_some() || self.due_date_picker.is_some() || self.quick_capture_open
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
        let options = due_date_picker_options(Utc::now().date_naive());
        let highlighted = options
            .iter()
            .position(|(_, date)| *date == current_due)
            .unwrap_or(0);
        self.due_date_picker = Some(DueDatePickerState {
            task_id,
            highlighted,
        });
    }

    pub fn close_due_date_picker(&mut self) {
        self.due_date_picker = None;
    }

    pub fn move_due_date_picker_highlight(&mut self, delta: i32) {
        let Some(picker) = &mut self.due_date_picker else {
            return;
        };
        let max = due_date_picker_options(Utc::now().date_naive()).len() as i32 - 1;
        picker.highlighted = (picker.highlighted as i32 + delta).clamp(0, max) as usize;
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
        let options = due_date_picker_options(Utc::now().date_naive());
        let Some((_, due_date)) = options.get(index) else {
            return;
        };
        let due_date = *due_date;

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

    pub fn delete_task(&mut self, id: TaskId) {
        if let Err(e) = task_repo::delete(&self.conn, id) {
            self.error_message = Some(e.to_string());
            return;
        }
        if self.selection == Selection::Task(id) {
            self.clear_selection();
        }
        self.refresh_visible_tasks();
    }

    pub fn delete_project(&mut self, id: ProjectId) {
        if let Err(e) = project_repo::delete(&self.conn, id) {
            self.error_message = Some(e.to_string());
            return;
        }
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
    }

    pub fn delete_tag(&mut self, id: TagId) {
        if let Err(e) = tag_repo::delete(&self.conn, id) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_tags();
        if self.perspective == Perspective::Tag(id) {
            // The perspective we were viewing no longer exists; fall back to Inbox
            // (this also clears selection/edit buffers and refreshes the task list).
            self.set_perspective(Perspective::Inbox);
        } else {
            if self.selection == Selection::Tag(id) {
                self.clear_selection();
            }
            self.refresh_visible_tasks();
        }
    }

    pub fn save_task_edits(&mut self) {
        let Some(buf) = &self.task_edit_buffer else {
            return;
        };
        let mut tag_ids = Vec::new();
        for name in &buf.tag_names {
            match tag_repo::get_or_create_by_name(&self.conn, name) {
                Ok(tag) => tag_ids.push(tag.id),
                Err(e) => {
                    self.error_message = Some(e.to_string());
                    return;
                }
            }
        }
        let task = Task {
            id: buf.id,
            title: buf.title.clone(),
            notes: buf.notes.clone(),
            project_id: buf.project_id,
            due_date: buf.due_date,
            defer_date: buf.defer_date,
            flagged: buf.flagged,
            completed: buf.completed,
            completed_at: buf.completed_at,
            created_at: buf.created_at,
            estimated_minutes: buf.estimated_minutes,
            tags: tag_ids,
        };
        if let Err(e) = task_repo::update(&self.conn, &task) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_visible_tasks();
        self.refresh_tags();
    }

    pub fn add_tag_to_edit_buffer(&mut self) {
        if let Some(buf) = &mut self.task_edit_buffer {
            let name = buf.new_tag_input.trim().to_string();
            if !name.is_empty() && !buf.tag_names.contains(&name) {
                buf.tag_names.push(name);
            }
            buf.new_tag_input.clear();
        }
        self.save_task_edits();
    }

    pub fn remove_tag_from_edit_buffer(&mut self, name: &str) {
        if let Some(buf) = &mut self.task_edit_buffer {
            buf.tag_names.retain(|t| t != name);
        }
        self.save_task_edits();
    }

    pub fn toggle_complete(&mut self, id: TaskId, completed: bool) {
        if let Err(e) = task_repo::set_completed(&self.conn, id, completed) {
            self.error_message = Some(e.to_string());
        }
        self.refresh_visible_tasks();
        if let Some(buf) = &self.task_edit_buffer
            && buf.id == id
        {
            self.select_task(id);
        }
    }

    pub fn toggle_flag(&mut self, id: TaskId, flagged: bool) {
        if let Err(e) = task_repo::set_flagged(&self.conn, id, flagged) {
            self.error_message = Some(e.to_string());
        }
        self.refresh_visible_tasks();
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
        };
        if let Err(e) = project_repo::update(&self.conn, &project) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_projects();
    }

    pub fn create_tag(&mut self) {
        let name = self.new_tag_name.trim();
        if name.is_empty() {
            return;
        }
        if self.tags.iter().any(|t| t.name == name) {
            self.error_message = Some(format!("Tag \"{name}\" already exists"));
            return;
        }
        let tag = Tag {
            id: TagId::new(),
            name: name.to_string(),
        };
        if let Err(e) = tag_repo::create(&self.conn, &tag) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.new_tag_name.clear();
        self.refresh_tags();
    }

    pub fn save_tag_edits(&mut self) {
        let Some(buf) = &self.tag_edit_buffer else {
            return;
        };
        if self
            .tags
            .iter()
            .any(|t| t.id != buf.id && t.name == buf.name)
        {
            self.error_message = Some(format!("Tag \"{}\" already exists", buf.name));
            return;
        }
        let tag = Tag {
            id: buf.id,
            name: buf.name.clone(),
        };
        if let Err(e) = tag_repo::update(&self.conn, &tag) {
            self.error_message = Some(e.to_string());
            return;
        }
        self.refresh_tags();
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
    fn quick_capture_defaults_to_inbox() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.quick_entry_buffer = "buy milk".to_string();
        state.quick_capture_submit();

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].title, "buy milk");
        assert!(state.visible_tasks[0].project_id.is_none());
    }

    #[test]
    fn quick_capture_popup_opens_submits_and_closes() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        assert!(!state.quick_capture_open);

        state.open_quick_capture();
        assert!(state.quick_capture_open);
        assert!(state.any_picker_open());

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
    fn quick_capture_tags_current_tag() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_tag_name = "errand".to_string();
        state.create_tag();
        let tag_id = state.tags[0].id;

        state.set_perspective(Perspective::Tag(tag_id));
        state.quick_entry_buffer = "buy stamps".to_string();
        state.quick_capture_submit();

        assert_eq!(state.visible_tasks.len(), 1);
        assert_eq!(state.visible_tasks[0].tags, vec![tag_id]);
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
        let expected = Utc::now().date_naive() + Duration::days(1);
        let updated = task_repo::get(&state.conn, task_id).unwrap().unwrap();
        assert_eq!(updated.due_date, Some(expected));
    }

    #[test]
    fn sidebar_entries_lists_projects_and_tags_in_render_order() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.new_project_name = "Kitchen Remodel".to_string();
        state.create_project();
        state.new_tag_name = "errand".to_string();
        state.create_tag();
        let project_id = state.projects[0].id;
        let tag_id = state.tags[0].id;

        let entries = state.sidebar_entries();
        assert_eq!(
            entries,
            vec![
                Perspective::Inbox,
                Perspective::Today,
                Perspective::Flagged,
                Perspective::Completed,
                Perspective::AllProjects,
                Perspective::Project(project_id),
                Perspective::AllTags,
                Perspective::Tag(tag_id),
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
        state.toggle_flag(state.visible_tasks[1].id, true);
        let flagged_task = state.visible_tasks[1].id;

        state.focus_sidebar(0); // Inbox
        assert_eq!(state.highlighted_task, Some(inbox_task));

        state.move_sidebar_highlight(1); // Today (empty)
        assert!(state.highlighted_task.is_none());

        state.move_sidebar_highlight(1); // Flagged
        assert_eq!(state.perspective, Perspective::Flagged);
        assert_eq!(state.highlighted_task, Some(flagged_task));
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
}
