use eframe::egui;

/// Primary accent: used for selection, focused borders, and the sidebar
/// heading.
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(88, 166, 255);
/// Warm secondary accent: used for the AI chat panel and warning text.
pub const WARM: egui::Color32 = egui::Color32::from_rgb(240, 165, 55);
/// Warm/red color for overdue due dates.
pub const OVERDUE: egui::Color32 = egui::Color32::from_rgb(235, 108, 108);

const BASE: egui::Color32 = egui::Color32::from_rgb(22, 24, 28);
const PANEL: egui::Color32 = egui::Color32::from_rgb(27, 29, 34);
const SIDEBAR: egui::Color32 = egui::Color32::from_rgb(24, 26, 31);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(34, 37, 43);
const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(42, 45, 52);

/// A barely-there hairline for separators, window borders and popups — a
/// translucent white overlay rather than a flat gray, so it reads as a soft
/// seam between two surfaces (OmniFocus-style) instead of a hard drawn line,
/// and stays correctly subtle no matter which of the panel colors above it
/// sits on.
const HAIRLINE: egui::Color32 = egui::Color32::from_rgba_premultiplied(14, 14, 14, 14);

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
    // A soft tint rather than a saturated block, with selected text left
    // near-white instead of accent-colored — the highlighted *band* carries
    // the selection, not a loud outline or colored label (OmniFocus keeps
    // row text neutral and only tints the background).
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.28);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_gray(235));
    visuals.panel_fill = PANEL;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BASE;
    visuals.faint_bg_color = egui::Color32::from_rgb(30, 32, 38);
    visuals.error_fg_color = OVERDUE;
    visuals.warn_fg_color = WARM;

    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.weak_bg_fill = PANEL;
    // Separators/indentation lines: a soft hairline instead of the default
    // flat mid-gray.
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, HAIRLINE);
    visuals.widgets.inactive.weak_bg_fill = SURFACE;
    visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    // Hover/focus outlines: present but gentle, not a bright full-strength line.
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40));
    visuals.widgets.active.weak_bg_fill = ACCENT.gamma_multiply(0.4);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.7));

    // Windows/popups/menus: same soft hairline border, and a lower, more
    // diffuse shadow than egui's default (which is tuned for a strong
    // "floating card" look) — closer to the gentle elevation OmniFocus uses
    // for its inspector and popovers.
    visuals.window_stroke = egui::Stroke::new(1.0, HAIRLINE);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(60),
    };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 3],
        blur: 14,
        spread: 0,
        color: egui::Color32::from_black_alpha(60),
    };

    for widgets in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widgets.corner_radius = 8u8.into();
    }
    visuals.window_corner_radius = 12u8.into();
    visuals.menu_corner_radius = 10u8.into();

    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.window_margin = 16i8.into();
    style.spacing.menu_margin = 8i8.into();
    style.spacing.interact_size.y = 28.0;

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

/// A `Frame` for the AI chat panel: a soft top seam plus a fill distinct
/// from both the sidebar and central content, so it reads as its own
/// full-width strip rather than an extension of the task list above it — a
/// faint accent tint rather than a hard, fully-saturated rule.
pub fn chat_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style)
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.3)))
}
