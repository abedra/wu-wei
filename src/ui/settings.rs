use eframe::egui;

use crate::llm::ProviderKind;
use crate::state::AppState;

/// Fixed id so a future shortcut could focus this field directly (mirrors
/// `quick_capture::field_id`/`new_project::field_id`).
pub fn api_key_field_id() -> egui::Id {
    egui::Id::new("settings_api_key_field")
}

/// A hand-maintained mirror of the shortcuts wired up in `ui::shortcuts` —
/// that module has no self-describing registry to generate this list from,
/// so keep the two in sync by hand when a shortcut is added or changed.
const SHORTCUT_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        "Views",
        &[
            ("Cmd+1", "Inbox"),
            ("Cmd+2", "Today"),
            ("Cmd+3", "Completed"),
            ("Cmd+4", "Overdue"),
            ("Cmd+,", "Settings"),
        ],
    ),
    (
        "Navigation",
        &[
            (
                "Up / Down",
                "Move the task cursor (or the sidebar selection, once focused)",
            ),
            ("Left", "Move keyboard focus to the sidebar"),
            ("Tab", "Hand Up/Down from the sidebar back to the task list"),
        ],
    ),
    (
        "Tasks",
        &[
            ("Enter", "Toggle the detail panel for the highlighted task"),
            ("Space", "Toggle complete"),
            ("M", "Move to project"),
            ("D", "Set due date"),
            ("Cmd+Backspace", "Delete the highlighted task"),
        ],
    ),
    (
        "Creating",
        &[
            ("Cmd+N", "New task (quick capture)"),
            (
                "Shift+Enter",
                "In quick capture: add literally, skipping AI parsing",
            ),
            ("Cmd+Shift+N", "New project"),
        ],
    ),
    ("Sync", &[("Cmd+Shift+S", "Sync now")]),
    ("Any popup", &[("Enter", "Confirm"), ("Esc", "Cancel")]),
];

/// Draws the Settings screen when open (toggled by Cmd+, — see
/// `ui::shortcuts`). Same floating-window shape as the other popups, but
/// commits via an explicit Save rather than live-as-you-type: applying a
/// half-typed db path on every keystroke would be actively harmful.
pub fn draw(ctx: &egui::Context, state: &mut AppState) {
    if state.settings.is_none() {
        return;
    }

    let mut save_clicked = false;
    let mut cancel_clicked = false;

    egui::Window::new("Settings")
        .collapsible(false)
        .resizable(true)
        .default_width(440.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            let draft = state.settings.as_mut().unwrap();

            ui.heading("AI / LLM");
            ui.label(
                "Powers AI-assisted quick capture and the chat panel. \
                 Leave the API key blank to disable both.",
            );
            ui.add_space(4.0);

            egui::ComboBox::from_label("Provider")
                .selected_text(match draft.llm_provider {
                    ProviderKind::OpenAi => "OpenAI",
                    ProviderKind::Anthropic => "Anthropic",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut draft.llm_provider, ProviderKind::OpenAi, "OpenAI");
                    ui.selectable_value(
                        &mut draft.llm_provider,
                        ProviderKind::Anthropic,
                        "Anthropic",
                    );
                });

            ui.horizontal(|ui| {
                ui.label("API key");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.llm_api_key)
                        .password(!draft.show_api_key)
                        .id(api_key_field_id())
                        .desired_width(260.0),
                );
                if ui
                    .selectable_label(draft.show_api_key, "👁")
                    .on_hover_text(if draft.show_api_key {
                        "Hide API key"
                    } else {
                        "Show API key"
                    })
                    .clicked()
                {
                    draft.show_api_key = !draft.show_api_key;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Base URL");
                ui.add(egui::TextEdit::singleline(&mut draft.llm_base_url).desired_width(260.0));
            });
            ui.horizontal(|ui| {
                ui.label("Model");
                ui.add(egui::TextEdit::singleline(&mut draft.llm_model).desired_width(260.0));
            });

            ui.separator();
            ui.heading("Database");
            ui.horizontal(|ui| {
                ui.label("File path");
                ui.add(egui::TextEdit::singleline(&mut draft.db_path).desired_width(300.0));
            });
            ui.weak("Changing this reconnects immediately on Save.");

            ui.separator();
            ui.heading("Sync");
            ui.label(
                "Reconciles this device's tasks/projects against others via a \
                 shared folder (Dropbox, iCloud Drive, a network share, a USB \
                 stick, ...) every device points at the same place — not a live \
                 shared database. Leave blank to disable.",
            );
            ui.horizontal(|ui| {
                ui.label("Folder");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.sync_folder_path).desired_width(300.0),
                );
            });
            if let Some(status) = &state.sync_status {
                ui.weak(format!("Last sync: {status}"));
            }

            ui.separator();
            ui.heading("Keyboard Shortcuts");
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for (group, entries) in SHORTCUT_GROUPS {
                        ui.strong(*group);
                        egui::Grid::new(("settings_shortcuts", *group))
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                for (keys, desc) in *entries {
                                    ui.monospace(*keys);
                                    ui.label(*desc);
                                    ui.end_row();
                                }
                            });
                        ui.add_space(6.0);
                    }
                });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    save_clicked = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel_clicked = true;
                }
            });
        });

    if save_clicked {
        state.save_settings();
    } else if cancel_clicked {
        state.close_settings();
    }
}
