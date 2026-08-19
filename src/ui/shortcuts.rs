use eframe::egui;

use crate::state::{AppState, Perspective};
use crate::ui::{new_project, quick_capture};

/// Global, OmniFocus-inspired keyboard shortcuts. Called once per frame, before
/// any panel is drawn, so a shortcut fires regardless of which panel has focus.
pub fn handle(ctx: &egui::Context, state: &mut AppState) {
    // `consume_shortcut` ignores extra Shift/Alt modifiers when matching, so
    // Cmd-Shift-N must be checked before the less-specific Cmd-N — otherwise
    // Cmd-N would consume the keypress first and the new-project popup could
    // never open (see egui's `InputState::consume_shortcut` docs).
    handle_new_project_popup(ctx, state);
    handle_quick_capture(ctx, state);
    handle_project_picker(ctx, state);
    handle_due_date_picker(ctx, state);
    handle_sidebar_navigation(ctx, state);
    handle_focus_sidebar(ctx, state);
    handle_task_navigation(ctx, state);
    handle_toggle_details(ctx, state);
    handle_perspective_switches(ctx, state);
    handle_delete(ctx, state);
    handle_complete_toggle(ctx, state);
}

/// While the sidebar has keyboard control (entered via `handle_focus_sidebar`
/// below, or by clicking a row), Up/Down step through perspectives, switching
/// live. Each step also places the keyboard inside that perspective's content
/// immediately (`AppState::focus_sidebar`/`move_sidebar_highlight` highlight
/// its first task) — Space/Enter/M/D/delete all act on it right away, no
/// extra step required. Tab still hands Up/Down themselves over to the
/// content list, to move past that first item.
fn handle_sidebar_navigation(ctx: &egui::Context, state: &mut AppState) {
    if !state.sidebar_focused {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
        state.move_sidebar_highlight(1);
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
        state.move_sidebar_highlight(-1);
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)) {
        state.exit_sidebar_focus();
    }
}

/// Left arrow moves keyboard control to the sidebar, highlighting whichever
/// perspective is currently active. This is the sole entry point into sidebar
/// navigation from the keyboard — deliberately not tied to egui's native
/// Tab-focus landing on a sidebar button, which proved unreliable (focus
/// timing between a click and a `gained_focus` check could desync which row
/// Up/Down actually moved).
fn handle_focus_sidebar(ctx: &egui::Context, state: &mut AppState) {
    if state.sidebar_focused || state.any_picker_open() {
        return;
    }
    if ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
        let entries = state.sidebar_entries();
        let index = entries
            .iter()
            .position(|&p| p == state.perspective)
            .unwrap_or(0);
        state.focus_sidebar(index);
    }
}

fn handle_perspective_switches(ctx: &egui::Context, state: &mut AppState) {
    const PERSPECTIVE_KEYS: [(egui::Key, Perspective); 5] = [
        (egui::Key::Num1, Perspective::Inbox),
        (egui::Key::Num2, Perspective::AllTags),
        (egui::Key::Num3, Perspective::Today),
        (egui::Key::Num4, Perspective::Completed),
        (egui::Key::Num5, Perspective::Overdue),
    ];
    for (key, perspective) in PERSPECTIVE_KEYS {
        let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, key);
        if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
            state.set_perspective(perspective);
        }
    }
}

/// Cmd+N pops open the quick-capture window and focuses its field. While
/// open, it owns Enter (submit, parsed by AI when one is configured — see
/// `AppState::quick_capture_submit_with_ai`) and Escape (cancel) — this is
/// why it's checked first in `handle`, ahead of the other any-picker-open
/// handlers.
fn handle_quick_capture(ctx: &egui::Context, state: &mut AppState) {
    if state.quick_capture_open {
        if state.llm_busy {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            state.quick_capture_submit_with_ai();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.close_quick_capture();
        }
        return;
    }

    if state.project_picker.is_some() || state.due_date_picker.is_some() {
        return;
    }
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N);
    if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
        state.open_quick_capture();
        ctx.memory_mut(|m| m.request_focus(quick_capture::field_id()));
    }
}

/// Cmd+Shift+N pops open the new-project popup and focuses its field. Same
/// open/submit/cancel shape as `handle_quick_capture`.
fn handle_new_project_popup(ctx: &egui::Context, state: &mut AppState) {
    if state.new_project_popup_open {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            state.submit_new_project_popup();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.close_new_project_popup();
        }
        return;
    }

    if state.project_picker.is_some() || state.due_date_picker.is_some() || state.quick_capture_open
    {
        return;
    }
    let shortcut = egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::N,
    );
    if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
        state.open_new_project_popup();
        ctx.memory_mut(|m| m.request_focus(new_project::field_id()));
    }
}

fn handle_delete(ctx: &egui::Context, state: &mut AppState) {
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Backspace);
    if !ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
        return;
    }
    if state.any_picker_open() {
        return;
    }
    if let Some(id) = state.highlighted_task {
        state.delete_task(id);
    }
}

/// Up/Down arrows move the keyboard cursor over the task list, mirroring
/// OmniFocus: navigation does not open the detail panel by itself (see
/// [`handle_toggle_details`] for that). Skipped while a text field has focus
/// (so it doesn't fight cursor movement) or while a picker is open (it owns
/// arrows then, see [`handle_project_picker`]/[`handle_due_date_picker`]).
fn handle_task_navigation(ctx: &egui::Context, state: &mut AppState) {
    if state.any_picker_open() || state.sidebar_focused {
        return;
    }
    if ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
        state.move_highlight(1);
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
        state.move_highlight(-1);
    }
}

/// Enter opens the detail panel for the highlighted task, or closes it again
/// if it's already showing that same task. Guarded like Space, below, so it
/// doesn't hijack Enter from text fields (quick capture, tag input, ...).
fn handle_toggle_details(ctx: &egui::Context, state: &mut AppState) {
    if state.any_picker_open() {
        return;
    }
    if ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        state.toggle_highlighted_task_details();
    }
}

/// `M` opens a keyboard-driven "move to project" picker for the highlighted
/// task (guarded like Space, below). While open, it takes over arrows/Enter/
/// Escape so a task can be filed into a project without touching the mouse.
fn handle_project_picker(ctx: &egui::Context, state: &mut AppState) {
    if state.project_picker.is_some() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            state.move_project_picker_highlight(1);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            state.move_project_picker_highlight(-1);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            state.confirm_project_picker();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.close_project_picker();
        }
        return;
    }

    if state.any_picker_open() || ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::M))
        && state.highlighted_task.is_some()
    {
        state.open_project_picker();
    }
}

/// `D` opens a keyboard-driven due-date picker for the highlighted task, with
/// quick relative options (Today, Tomorrow, This Weekend, Next Week, or no
/// date). Same open/navigate/confirm/cancel shape as [`handle_project_picker`].
fn handle_due_date_picker(ctx: &egui::Context, state: &mut AppState) {
    if state.due_date_picker.is_some() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            state.move_due_date_picker_highlight(1);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            state.move_due_date_picker_highlight(-1);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            state.confirm_due_date_picker();
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.close_due_date_picker();
        }
        return;
    }

    if state.any_picker_open() || ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::D))
        && state.highlighted_task.is_some()
    {
        state.open_due_date_picker();
    }
}

fn handle_complete_toggle(ctx: &egui::Context, state: &mut AppState) {
    if state.any_picker_open() {
        return;
    }
    // Unlike the shortcuts above, Space has no modifier, so it would eat spaces
    // typed into text fields. Only consume it when no widget owns keyboard focus.
    if ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    let space_pressed = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space));
    if !space_pressed {
        return;
    }
    if let Some(id) = state.highlighted_task
        && let Some(task) = state.visible_tasks.iter().find(|t| t.id == id)
    {
        state.toggle_complete(id, !task.completed);
    }
}
