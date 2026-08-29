use eframe::egui;

use crate::state::{AppState, Perspective};

/// Global, OmniFocus-inspired keyboard shortcuts. Called once per frame, before
/// any panel is drawn, so a shortcut fires regardless of which panel has focus.
pub fn handle(ctx: &egui::Context, state: &mut AppState) {
    // `consume_shortcut` ignores extra Shift/Alt modifiers when matching, so
    // Cmd-Shift-N must be checked before the less-specific Cmd-N — otherwise
    // Cmd-N would consume the keypress first and the new-project popup could
    // never open (see egui's `InputState::consume_shortcut` docs).
    handle_new_project_popup(ctx, state);
    handle_quick_capture(ctx, state);
    handle_settings(ctx, state);
    handle_sync(ctx, state);
    handle_archive_confirm(ctx, state);
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

/// Cmd+, opens Settings — the conventional "Preferences" shortcut. Unlike
/// the other popups, Settings has no single Enter-to-confirm action (it's a
/// form with several fields, committed via the Save button in
/// `ui::settings`), so this only takes over Escape-to-cancel while open.
fn handle_settings(ctx: &egui::Context, state: &mut AppState) {
    if state.settings.is_some() {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.close_settings();
        }
        return;
    }

    if state.any_picker_open() {
        return;
    }
    let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Comma);
    if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
        state.open_settings();
    }
}

/// Cmd+Shift+S triggers a sync directly (see `ui::sidebar`'s ⟳ button /
/// `AppState::run_sync`) — no popup involved, so unlike the other shortcuts
/// here there's nothing to gate on `any_picker_open`.
fn handle_sync(ctx: &egui::Context, state: &mut AppState) {
    let shortcut = egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
        egui::Key::S,
    );
    if ctx.input_mut(|i| i.consume_shortcut(&shortcut)) {
        state.run_sync();
    }
}

/// Enter confirms, Escape cancels — only reachable by clicking the Archive
/// Completed button (see `ui::task_list`), so there's no "open" shortcut to
/// handle here, unlike the other popups.
fn handle_archive_confirm(ctx: &egui::Context, state: &mut AppState) {
    if !state.archive_confirm_open {
        return;
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        state.confirm_archive_completed();
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        state.close_archive_confirm();
    }
}

/// While the sidebar has keyboard control (entered via `handle_focus_sidebar`
/// below, or by clicking a row), Up/Down step through perspectives, switching
/// live. Each step also places the keyboard inside that perspective's content
/// immediately (`AppState::focus_sidebar`/`move_sidebar_highlight` highlight
/// its first task) — Space/Enter/M/D/delete all act on it right away, no
/// extra step required. Right arrow or Tab hand control back to the content
/// list, mirroring `handle_focus_sidebar`'s Left-arrow/Tab entry the other
/// way — either key moves *between* the two sections; only Up/Down move
/// *within* whichever one currently has control.
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
    let exit = ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
            || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
    });
    if exit {
        state.exit_sidebar_focus();
    }
}

/// Left arrow or Tab moves keyboard control from the content list to the
/// sidebar, highlighting whichever perspective is currently active — the
/// sole entry point into sidebar navigation from the keyboard, deliberately
/// not tied to egui's native Tab-focus landing on a sidebar button, which
/// proved unreliable (focus timing between a click and a `gained_focus`
/// check could desync which row Up/Down actually moved). Both keys are
/// skipped whenever a real widget already holds keyboard focus (a text
/// field, a button, ...), so egui's native per-field Tab-cycling still works
/// inside forms (Settings, task details, quick capture, ...) instead of
/// always jumping to the sidebar — the task list's own keyboard cursor
/// (`AppState::highlighted_task`) isn't real egui focus, so this fires for
/// it, which is the case it's actually meant to cover: swapping sections
/// instead of quietly landing focus on whatever button happens to sit
/// nearest in the widget tree.
fn handle_focus_sidebar(ctx: &egui::Context, state: &mut AppState) {
    if state.sidebar_focused || state.any_picker_open() {
        return;
    }
    if ctx.memory(|m| m.focused()).is_some() {
        return;
    }
    let enter = ctx.input_mut(|i| {
        i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
            || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
    });
    if enter {
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
        (egui::Key::Num2, Perspective::Today),
        (egui::Key::Num3, Perspective::Completed),
        (egui::Key::Num4, Perspective::Overdue),
        (egui::Key::Num5, Perspective::Review),
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
/// `AppState::quick_capture_submit_with_ai`), Shift+Enter (submit literally,
/// bypassing AI parsing even when one is configured — see
/// `AppState::quick_capture_submit`), and Escape (cancel) — this is why it's
/// checked first in `handle`, ahead of the other any-picker-open handlers.
fn handle_quick_capture(ctx: &egui::Context, state: &mut AppState) {
    if state.quick_capture_open {
        if state.llm_busy {
            return;
        }
        // `consume_key` ignores extra Shift/Alt modifiers when matching (like
        // `consume_shortcut`), so Shift+Enter must be checked before the
        // less-specific plain Enter — otherwise plain Enter would consume the
        // keypress first and Shift+Enter could never fire.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter)) {
            state.quick_capture_submit();
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
/// doesn't hijack Enter from text fields (quick capture, ...).
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
/// date) plus a free-text field for a typed phrase resolved by AI. Same
/// open/navigate/confirm/cancel shape as [`handle_project_picker`].
fn handle_due_date_picker(ctx: &egui::Context, state: &mut AppState) {
    if state.due_date_picker.is_some() {
        // Escape always bails, even mid-resolve.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            state.close_due_date_picker();
            return;
        }
        // While the typed phrase is being resolved, or while the text field
        // has keyboard focus, the option list doesn't own the arrows/Enter —
        // the field does (its own Enter-submit lives in `ui::due_date_picker`).
        if state.due_date_picker_ai_busy() || ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
            state.move_due_date_picker_highlight(1);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
            state.move_due_date_picker_highlight(-1);
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            state.confirm_due_date_picker();
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
