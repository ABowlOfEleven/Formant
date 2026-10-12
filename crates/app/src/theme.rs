//! Cyan + ember HUD theme - a sibling to NeonPrime's palette.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2,
};

pub const BG: Color32 = Color32::from_rgb(7, 10, 14);
pub const PANEL: Color32 = Color32::from_rgb(13, 18, 24);
pub const CARD: Color32 = Color32::from_rgb(18, 25, 32);
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

/// Give egui a fallback font with broad symbol coverage. Ubuntu-Light (egui's
/// default) is missing some glyphs we use (the close X, arrows, geometric dots),
/// which otherwise render as empty boxes ("tofu"). Windows always ships a font
/// that covers them; this is best-effort and skips silently if none is found.
fn install_symbol_fallback(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\seguisym.ttf", // Segoe UI Symbol: shapes, dingbats, arrows
        r"C:\Windows\Fonts\segoeui.ttf",  // Segoe UI
        r"C:\Windows\Fonts\arial.ttf",
    ];
    let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "win_symbols".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    // Append as the last fallback so it only fills glyphs the primary font lacks.
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("win_symbols".to_owned());
    }
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context) {
    install_symbol_fallback(ctx);
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
    // Stock widgets follow the hardware look of the hand-painted ones: buttons
    // are metal caps with a dark seam around them, text fields and dropdown
    // wells are recessed glass.
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    v.widgets.inactive.bg_fill = Color32::from_rgb(56, 62, 70);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(56, 62, 70);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_black_alpha(200));
    v.widgets.hovered.bg_fill = Color32::from_rgb(68, 75, 84);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(68, 75, 84);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, lerp(Color32::BLACK, CYAN, 0.45));
    v.widgets.active.bg_fill = Color32::from_rgb(38, 42, 48);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(38, 42, 48);
    v.widgets.active.fg_stroke = Stroke::new(1.0, CYAN);
    v.widgets.active.bg_stroke = Stroke::new(1.0, CYAN);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(38, 42, 48);
    v.extreme_bg_color = Color32::from_rgb(7, 10, 12);
    v.text_edit_bg_color = Some(Color32::from_rgb(7, 10, 12));

    v.selection.bg_fill = CYAN.gamma_multiply(0.30);
    v.selection.stroke = Stroke::new(1.0, CYAN);
    v.hyperlink_color = CYAN;
    v.slider_trailing_fill = true;

    ctx.set_global_style(style);
}

/// A themed card frame for grouping controls.
///
/// Cards are plain metal plates (a solid fill, since a frame can't carry a
/// gradient) with the same dark seam and drop shadow as the hand-painted
/// faceplates, so panels that still use them sit in the same rack.
pub fn card(style: &egui::Style) -> egui::Frame {
    egui::Frame::group(style)
        .fill(Color32::from_rgb(36, 40, 46))
        .stroke(Stroke::new(1.0, Color32::from_black_alpha(220)))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(14))
        .shadow(egui::epaint::Shadow { offset: [0, 5], blur: 14, spread: 0, color: Color32::from_black_alpha(140) })
}
