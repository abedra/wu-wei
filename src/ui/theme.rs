use eframe::egui;

/// Primary accent: used for selection, flags, focused borders, and the
/// sidebar heading.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(88, 166, 255);
/// Warm color for flagged tasks.
pub const FLAG: egui::Color32 = egui::Color32::from_rgb(240, 165, 55);
/// Warm/red color for overdue due dates.
pub const OVERDUE: egui::Color32 = egui::Color32::from_rgb(235, 108, 108);

const BASE: egui::Color32 = egui::Color32::from_rgb(22, 24, 28);
const PANEL: egui::Color32 = egui::Color32::from_rgb(27, 29, 34);
const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(24, 26, 31);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(34, 37, 43);
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(42, 45, 52);

/// Applies a single, consistent dark theme (blue accent) across the whole
/// app. Called once at startup; every panel/window inherits it automatically.
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        apply_to_style(style);
    });
}

fn apply_to_style(style: &mut egui::Style) {
    let mut visuals = egui::Visuals::dark();

    visuals.hyperlink_color = ACCENT;
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.5);
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.panel_fill = PANEL;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BASE;
    visuals.faint_bg_color = egui::Color32::from_rgb(30, 32, 38);
    visuals.error_fg_color = OVERDUE;
    visuals.warn_fg_color = FLAG;

    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL;
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    visuals.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.4);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);

    for widgets in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widgets.corner_radius = 6u8.into();
    }
    visuals.window_corner_radius = 10u8.into();
    visuals.menu_corner_radius = 8u8.into();

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.window_margin = 14i8.into();
    style.spacing.menu_margin = 8i8.into();
    style.spacing.interact_size.y = 26.0;

    if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
        font.size = 19.0;
    }
    if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Body) {
        font.size = 14.0;
    }
    if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Button) {
        font.size = 14.0;
    }
}

/// A `Frame` for the sidebar panel with a slightly different fill than the
/// central content, so the two read as distinct regions.
pub fn sidebar_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).fill(SIDEBAR)
}

/// A `Frame` for the AI chat panel: a top accent border plus a fill distinct
/// from both the sidebar and central content, so it reads as its own
/// full-width strip rather than an extension of the task list above it.
pub fn chat_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style)
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.5)))
}
