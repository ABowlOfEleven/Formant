//! Cyan + ember HUD theme - a sibling to NeonPrime's palette.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(7, 10, 14);
pub const PANEL: Color32 = Color32::from_rgb(13, 18, 24);
pub const CARD: Color32 = Color32::from_rgb(18, 25, 32);
pub const CARD_HI: Color32 = Color32::from_rgb(25, 35, 44);
pub const LINE: Color32 = Color32::from_rgb(38, 52, 62);
pub const CYAN: Color32 = Color32::from_rgb(45, 212, 238);
pub const EMBER: Color32 = Color32::from_rgb(255, 138, 40);
pub const TEXT: Color32 = Color32::from_rgb(208, 226, 234);
pub const MUTED: Color32 = Color32::from_rgb(120, 146, 158);
pub const GOOD: Color32 = Color32::from_rgb(70, 222, 160);

/// Blend two colors (gamma-correct).
pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    a.lerp_to_gamma(b, t.clamp(0.0, 1.0))
}

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();

    // Spacing - more breathing room.
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(9.0, 9.0);
    s.button_padding = Vec2::new(12.0, 7.0);
    s.slider_width = 150.0;
    s.interact_size.y = 24.0;
    s.window_margin = Margin::same(14);
    s.menu_margin = Margin::same(8);

    // Typography.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(21.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0, LINE);

    let r = CornerRadius::same(7);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    v.widgets.inactive.bg_fill = CARD_HI;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.hovered.bg_fill = lerp(CARD_HI, CYAN, 0.10);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, CYAN);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, lerp(LINE, CYAN, 0.4));
    v.widgets.active.bg_fill = lerp(CARD_HI, CYAN, 0.18);
    v.widgets.active.fg_stroke = Stroke::new(1.0, CYAN);
    v.widgets.active.bg_stroke = Stroke::new(1.0, CYAN);

    v.selection.bg_fill = CYAN.gamma_multiply(0.30);
    v.selection.stroke = Stroke::new(1.0, CYAN);
    v.hyperlink_color = CYAN;
    v.slider_trailing_fill = true;

    ctx.set_global_style(style);
}

/// A themed card frame for grouping controls.
pub fn card(style: &egui::Style) -> egui::Frame {
    egui::Frame::group(style)
        .fill(CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(12))
}
