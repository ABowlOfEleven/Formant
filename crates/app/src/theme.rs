//! Cyan + ember HUD theme — a sibling to NeonPrime's palette.

use eframe::egui::{self, Color32, CornerRadius, Stroke};

pub const BG: Color32 = Color32::from_rgb(8, 12, 16);
pub const PANEL: Color32 = Color32::from_rgb(14, 20, 26);
pub const CARD: Color32 = Color32::from_rgb(19, 27, 34);
pub const CYAN: Color32 = Color32::from_rgb(34, 211, 238);
pub const EMBER: Color32 = Color32::from_rgb(255, 122, 24);
pub const TEXT: Color32 = Color32::from_rgb(202, 224, 232);
pub const MUTED: Color32 = Color32::from_rgb(120, 150, 162);
pub const GOOD: Color32 = Color32::from_rgb(60, 220, 160);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    {
        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = Some(TEXT);
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = BG;
        v.faint_bg_color = CARD;

        v.widgets.noninteractive.bg_fill = PANEL;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        v.widgets.inactive.bg_fill = Color32::from_rgb(26, 36, 44);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        v.widgets.hovered.bg_fill = Color32::from_rgb(34, 48, 58);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, CYAN);
        v.widgets.active.bg_fill = Color32::from_rgb(42, 60, 72);
        v.widgets.active.fg_stroke = Stroke::new(1.0, CYAN);

        v.selection.bg_fill = CYAN.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, CYAN);
        v.hyperlink_color = CYAN;

        let r = CornerRadius::same(5);
        v.widgets.noninteractive.corner_radius = r;
        v.widgets.inactive.corner_radius = r;
        v.widgets.hovered.corner_radius = r;
        v.widgets.active.corner_radius = r;
    }
    ctx.set_global_style(style);
}

/// A themed card frame for grouping controls.
pub fn card(style: &egui::Style) -> egui::Frame {
    egui::Frame::group(style)
        .fill(CARD)
        .stroke(Stroke::new(1.0, CYAN.gamma_multiply(0.45)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(10))
}
