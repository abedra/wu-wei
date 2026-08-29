use eframe::egui;

use crate::calendar;
use crate::llm::ProviderKind;
use crate::state::AppState;
use crate::ui::theme;

/// Fixed id so a future shortcut could focus this field directly (mirrors
/// `quick_capture::field_id`/`new_project::field_id`).
pub fn api_key_field_id() -> egui::Id {
    egui::Id::new("settings_api_key_field")
}

/// Draws a small checkmark glyph, painted as a stroked polyline rather than
/// a Unicode character — egui's bundled fonts don't cover U+2713, so a text
/// glyph falls back to a placeholder box instead of an actual checkmark.
fn checkmark_icon(ui: &mut egui::Ui, size: f32, color: egui::Color32) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let points = vec![
        egui::pos2(rect.left() + size * 0.15, rect.top() + size * 0.52),
        egui::pos2(rect.left() + size * 0.42, rect.top() + size * 0.78),
        egui::pos2(rect.left() + size * 0.85, rect.top() + size * 0.22),
    ];
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(2.0, color)));
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
            (
                "Left / Right",
                "Move keyboard focus between the sidebar and the task list",
            ),
            ("Tab", "Same as Left/Right, outside of a focused field"),
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

/// A file dialog seeded to open next to whatever `path` currently holds —
/// its parent directory if `path` names a file (possibly one that doesn't
/// exist yet), itself if it's already a directory, or the platform default
/// (e.g. home) if it's blank or doesn't resolve to anything on disk. Public
/// to `state`, which runs the actual (blocking) dialog call on a background
/// thread — see `AppState::browse_for_db_path`.
pub(crate) fn file_dialog_near(path: &str) -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new();
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return dialog;
    }
    let p = std::path::Path::new(trimmed);
    let dir = if p.is_dir() {
        Some(p)
    } else {
        p.parent().filter(|d| d.is_dir())
    };
    match dir {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}

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
    let mut browse_db_path_clicked = false;
    let mut browse_sync_folder_clicked = false;
    let mut connect_gcal_clicked = false;
    let mut disconnect_gcal_clicked = false;

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
                ui.add(egui::TextEdit::singleline(&mut draft.db_path).desired_width(240.0));
                let browsing = state.db_path_dialog.is_some();
                if ui
                    .add_enabled(!browsing, egui::Button::new("Browse…"))
                    .clicked()
                {
                    browse_db_path_clicked = true;
                }
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
                    egui::TextEdit::singleline(&mut draft.sync_folder_path).desired_width(240.0),
                );
                let browsing = state.sync_folder_dialog.is_some();
                if ui
                    .add_enabled(!browsing, egui::Button::new("Browse…"))
                    .clicked()
                {
                    browse_sync_folder_clicked = true;
                }
            });
            if let Some(status) = &state.sync_status {
                ui.weak(format!("Last sync: {status}"));
            }

            ui.separator();
            ui.heading("Calendar");
            ui.label(
                "Shows today's events from a Google Calendar in the Today view. \
                 Requires an OAuth client from your own Google Cloud project — \
                 see the README for setup steps.",
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Client ID");
                ui.add(egui::TextEdit::singleline(&mut draft.gcal_client_id).desired_width(260.0));
            });
            ui.horizontal(|ui| {
                ui.label("Client Secret");
                ui.add(
                    egui::TextEdit::singleline(&mut draft.gcal_client_secret)
                        .password(!draft.show_gcal_client_secret)
                        .desired_width(260.0),
                );
                if ui
                    .selectable_label(draft.show_gcal_client_secret, "👁")
                    .on_hover_text(if draft.show_gcal_client_secret {
                        "Hide client secret"
                    } else {
                        "Show client secret"
                    })
                    .clicked()
                {
                    draft.show_gcal_client_secret = !draft.show_gcal_client_secret;
                }
            });
            ui.horizontal(|ui| {
                if draft.gcal_connected {
                    // egui's horizontal layout reserves row height from
                    // `interact_size.y` before any widget is added; the
                    // button below actually renders taller than that (text
                    // height plus its own padding), so without this the
                    // checkmark/label get centered against the smaller
                    // guessed height and sit visibly above the button once
                    // it stretches the row.
                    let button_height = ui.text_style_height(&egui::TextStyle::Button)
                        + 2.0 * ui.spacing().button_padding.y;
                    ui.set_min_height(button_height);
                    checkmark_icon(ui, 14.0, theme::SUCCESS);
                    ui.colored_label(theme::ACCENT, "Connected");
                    if ui.button("Disconnect").clicked() {
                        disconnect_gcal_clicked = true;
                    }
                } else {
                    let ready = !draft.gcal_client_id.trim().is_empty()
                        && !draft.gcal_client_secret.trim().is_empty();
                    let connecting = state.google_auth_pending.is_some();
                    if ui
                        .add_enabled(
                            ready && !connecting,
                            egui::Button::new("Connect Google Calendar"),
                        )
                        .clicked()
                    {
                        connect_gcal_clicked = true;
                    }
                }
            });
            if let Some(status) = &state.google_auth_status {
                ui.weak(status);
            }
            ui.horizontal(|ui| {
                ui.label("Refresh every");
                ui.add(
                    egui::DragValue::new(&mut draft.gcal_refresh_minutes)
                        .speed(1.0)
                        .range(calendar::MIN_REFRESH_MINUTES..=calendar::MAX_REFRESH_MINUTES)
                        .suffix(" min"),
                );
            });
            ui.weak("How often the Today view re-fetches events while it's open.");

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
    if browse_db_path_clicked {
        state.browse_for_db_path();
    }
    if browse_sync_folder_clicked {
        state.browse_for_sync_folder();
    }
    if connect_gcal_clicked {
        state.start_google_oauth();
    }
    if disconnect_gcal_clicked {
        state.disconnect_google_calendar();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    #[test]
    fn draft_is_seeded_with_existing_settings_values() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        crate::db::settings_repo::set_many(
            &state.conn,
            &[
                ("llm_provider", "anthropic"),
                ("llm_api_key", "sk-test-123"),
                ("llm_base_url", "https://example.com"),
                ("llm_model", "claude-test"),
                ("sync_folder_path", "/home/test/sync"),
                ("gcal_client_id", "client-123"),
                ("gcal_client_secret", "shh"),
                ("gcal_refresh_token", "refresh-token"),
            ],
        )
        .unwrap();

        state.open_settings();
        let draft = state.settings.as_ref().unwrap();
        assert_eq!(draft.llm_api_key, "sk-test-123");
        assert_eq!(draft.llm_base_url, "https://example.com");
        assert_eq!(draft.llm_model, "claude-test");
        assert_eq!(draft.sync_folder_path, "/home/test/sync");
        assert!(!draft.db_path.is_empty());
        assert_eq!(draft.gcal_client_id, "client-123");
        assert_eq!(draft.gcal_client_secret, "shh");
        assert!(draft.gcal_connected);
    }

    #[test]
    fn draft_shows_disconnected_when_no_refresh_token_is_saved() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.open_settings();
        assert!(!state.settings.as_ref().unwrap().gcal_connected);
    }

    #[test]
    fn draws_without_panicking_when_open() {
        let mut state = AppState::new(crate::db::open_in_memory().unwrap());
        state.open_settings();
        assert!(state.settings.is_some());

        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(Default::default(), |ctx| {
            draw(ctx, &mut state);
        });
        output.textures_delta.clear();
    }
}
