//! Studio-hardware widgets: brushed faceplates, machined knobs with LED arcs,
//! LED push buttons, LED ladder meters and recessed screens.
//!
//! Everything here is painted by hand with egui's painter so it reads as
//! physical equipment rather than flat UI. Depth comes from three things used
//! consistently: one light source from above (lighter tops, darker bottoms,
//! highlight on upper edges, shadow under raised parts), real gradients built
//! as vertex-colored meshes, and lit elements that glow in the accent color
//! against dim, unlit versions of themselves.

use eframe::egui::{
    self, epaint::Shadow, Align2, Color32, CornerRadius, CursorIcon, FontId, Id, Margin, Mesh, Pos2, Rect,
    Response, Sense, Shape, Stroke, Ui, Vec2,
};

use crate::theme;

// ------------------------------------------------------------------ palette

/// Faceplate metal, top and bottom of the vertical gradient.
const METAL_TOP: Color32 = Color32::from_rgb(47, 52, 59);
const METAL_BOTTOM: Color32 = Color32::from_rgb(29, 33, 38);
/// Silkscreen print on the faceplate.
const SILK: Color32 = Color32::from_rgb(196, 207, 213);
/// Dimmer print for labels. Kept at 4.5:1 or better against every metal and
/// button surface it sits on (WCAG AA for small text).
const SILK_DIM: Color32 = Color32::from_rgb(160, 173, 180);
/// Keyboard focus ring.
const FOCUS: Color32 = Color32::from_rgb(120, 230, 255);
/// Glass of a screen or meter window.
const GLASS: Color32 = Color32::from_rgb(7, 10, 12);
/// Readout color on screens: warm, like an amber display.
pub const READOUT: Color32 = Color32::from_rgb(255, 176, 74);

fn black(a: u8) -> Color32 {
    Color32::from_black_alpha(a)
}
fn white(a: u8) -> Color32 {
    Color32::from_white_alpha(a)
}

// ------------------------------------------------------------------ primitives

/// A rectangle filled with a vertical gradient.
fn gradient_rect(rect: Rect, top: Color32, bottom: Color32) -> Shape {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    Shape::mesh(m)
}

/// A disc lit from above: `top` at its top edge fading to `bottom`.
fn gradient_disc(c: Pos2, r: f32, top: Color32, bottom: Color32) -> Shape {
    let mut m = Mesh::default();
    let n = 40;
    let mid = theme::lerp(top, bottom, 0.5);
    m.colored_vertex(c, mid);
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let p = c + Vec2::new(a.cos(), a.sin()) * r;
        let t = ((p.y - (c.y - r)) / (2.0 * r)).clamp(0.0, 1.0);
        m.colored_vertex(p, theme::lerp(top, bottom, t));
    }
    for i in 0..n {
        m.add_triangle(0, i + 1, i + 2);
    }
    Shape::mesh(m)
}

/// Points along a circular arc. Angles in radians, screen coordinates (y down,
/// so increasing angle runs clockwise on screen).
fn arc_points(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let steps = ((a1 - a0).abs() * r / 3.0).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32;
            c + Vec2::new(a.cos(), a.sin()) * r
        })
        .collect()
}

/// A soft shadow under a round part, faked with stacked translucent discs.
fn round_shadow(painter: &egui::Painter, c: Pos2, r: f32, drop: f32) {
    for (grow, a) in [(5.0, 18u8), (3.0, 34), (1.5, 60)] {
        painter.circle_filled(c + Vec2::new(0.0, drop), r + grow, black(a));
    }
}

/// A screw head with a slot at `angle`.
fn screw(painter: &egui::Painter, c: Pos2, angle: f32) {
    painter.circle_filled(c + Vec2::new(0.0, 0.8), 4.2, black(110));
    painter.add(gradient_disc(c, 3.6, Color32::from_rgb(120, 126, 133), Color32::from_rgb(52, 56, 62)));
    let d = Vec2::new(angle.cos(), angle.sin()) * 2.6;
    painter.line_segment([c - d, c + d], Stroke::new(1.2, Color32::from_rgb(28, 30, 34)));
}

/// Text that looks screen-printed onto metal: a dark offset under the ink so
/// it sits in the surface instead of floating on it.
pub fn silk_text(painter: &egui::Painter, pos: Pos2, align: Align2, text: &str, size: f32, color: Color32) -> Rect {
    let font = FontId::proportional(size);
    painter.text(pos + Vec2::new(0.0, 1.0), align, text, font.clone(), black(150));
    painter.text(pos, align, text, font, color)
}

/// A glowing LED dot.
fn led(painter: &egui::Painter, c: Pos2, r: f32, color: Color32, on: bool) {
    if on {
        for (grow, a) in [(r * 2.6, 0.10f32), (r * 1.6, 0.22)] {
            painter.circle_filled(c, grow, color.gamma_multiply(a));
        }
        painter.circle_filled(c, r, color);
        painter.circle_filled(c - Vec2::new(r * 0.3, r * 0.35), r * 0.4, white(150));
    } else {
        painter.circle_filled(c, r + 1.0, black(120));
        painter.circle_filled(c, r, theme::lerp(GLASS, color, 0.16));
    }
}

// ------------------------------------------------------------------ faceplate

/// A full-width rack panel: a [`faceplate`] stretched across the available width.
pub fn rack_panel<R>(
    ui: &mut Ui,
    title: &str,
    accent: Color32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    faceplate_impl(ui, Some(title), accent, true, add_contents)
}

/// A brushed-metal panel with bevelled edges, a drop shadow and corner screws,
/// sized to its contents. `title` is screen-printed across the top, with a
/// short accent strip under it.
pub fn faceplate<R>(
    ui: &mut Ui,
    title: Option<&str>,
    accent: Color32,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    faceplate_impl(ui, title, accent, false, add_contents)
}

fn faceplate_impl<R>(
    ui: &mut Ui,
    title: Option<&str>,
    accent: Color32,
    fill: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let bg = ui.painter().add(Shape::Noop);
    let top_pad = if title.is_some() { 30 } else { 16 };
    let frame = egui::Frame::new().inner_margin(Margin { left: 16, right: 16, top: top_pad, bottom: 16 });
    let inner = frame.show(ui, |ui| {
        if fill {
            ui.set_min_width(ui.available_width());
        }
        add_contents(ui)
    });
    let rect = inner.response.rect;
    let painter = ui.painter();
    let radius = CornerRadius::same(6);

    let mut shapes = vec![
        Shape::from(Shadow { offset: [0, 6], blur: 18, spread: 0, color: black(150) }.as_shape(rect, radius)),
        Shape::rect_filled(rect.expand(1.0), radius, black(200)),
        gradient_rect(rect.shrink(0.5), METAL_TOP, METAL_BOTTOM),
    ];
    // Brushed grain: faint horizontal streaks of slightly varying brightness.
    let mut y = rect.top() + 2.0;
    let mut seed = (rect.left() as u32).wrapping_mul(2654435761) ^ (rect.top() as u32);
    while y < rect.bottom() - 2.0 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let v = (seed >> 24) as u8;
        let col = if v & 1 == 0 { white(v % 7) } else { black(v % 12) };
        shapes.push(Shape::line_segment([Pos2::new(rect.left() + 1.0, y), Pos2::new(rect.right() - 1.0, y)], Stroke::new(1.0, col)));
        y += 1.0 + (v % 3) as f32;
    }
    // Bevel: light on the top and left edges, dark on the bottom and right.
    let (l, r, t, b) = (rect.left() + 0.5, rect.right() - 0.5, rect.top() + 0.5, rect.bottom() - 0.5);
    shapes.push(Shape::line_segment([Pos2::new(l + 4.0, t), Pos2::new(r - 4.0, t)], Stroke::new(1.0, white(48))));
    shapes.push(Shape::line_segment([Pos2::new(l, t + 4.0), Pos2::new(l, b - 4.0)], Stroke::new(1.0, white(18))));
    shapes.push(Shape::line_segment([Pos2::new(l + 4.0, b), Pos2::new(r - 4.0, b)], Stroke::new(1.0, black(140))));
    shapes.push(Shape::line_segment([Pos2::new(r, t + 4.0), Pos2::new(r, b - 4.0)], Stroke::new(1.0, black(90))));
    painter.set(bg, Shape::Vec(shapes));

    // Screws and title go on top of the metal (but under nothing the caller drew,
    // since they sit in the margins).
    let inset = 8.0;
    for (i, c) in [
        Pos2::new(rect.left() + inset, rect.top() + inset),
        Pos2::new(rect.right() - inset, rect.top() + inset),
        Pos2::new(rect.left() + inset, rect.bottom() - inset),
        Pos2::new(rect.right() - inset, rect.bottom() - inset),
    ]
    .into_iter()
    .enumerate()
    {
        screw(painter, c, 0.4 + i as f32 * 1.1 + rect.left() * 0.01);
    }
    if let Some(title) = title {
        let tr = silk_text(painter, Pos2::new(rect.left() + 22.0, rect.top() + 14.0), Align2::LEFT_CENTER, title, 12.5, SILK);
        let strip = Rect::from_min_size(Pos2::new(tr.left(), tr.bottom() + 3.0), Vec2::new(tr.width().min(28.0), 2.0));
        painter.rect_filled(strip, CornerRadius::same(1), accent);
    }
    inner
}

// ------------------------------------------------------------------ screen

/// A recessed glass window cut into the faceplate. `paint` draws the contents
/// inside the returned rect; the bezel and glass reflection go around and over.
/// `description` is what a screen reader announces for the display.
pub fn screen(ui: &mut Ui, size: Vec2, description: &str, paint: impl FnOnce(&egui::Painter, Rect)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, description));
    let painter = ui.painter_at(rect.expand(3.0));
    let r = CornerRadius::same(5);
    // Bezel: the cut is shadowed at the top and catches light at the bottom.
    painter.rect_filled(rect.expand(2.0), r, black(170));
    painter.line_segment([rect.left_bottom() + Vec2::new(4.0, 2.0), rect.right_bottom() + Vec2::new(-4.0, 2.0)], Stroke::new(1.0, white(30)));
    painter.rect_filled(rect, r, GLASS);
    paint(&painter.with_clip_rect(rect.shrink(1.0)), rect);
    // Inner shadow along the top edge, then a faint reflection on the glass.
    let shade = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 10.0f32.min(rect.height() * 0.3)));
    painter.add(gradient_rect(shade, black(140), black(0)));
    let gloss = Rect::from_min_max(rect.min + Vec2::new(2.0, 2.0), Pos2::new(rect.right() - 2.0, rect.center().y));
    painter.add(gradient_rect(gloss, white(9), white(0)));
    resp
}

// ------------------------------------------------------------------ knob

/// What a knob controls and how its travel maps onto the value.
#[derive(Clone, Copy)]
pub struct KnobSpec {
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    /// Logarithmic travel, for frequencies and times.
    pub log: bool,
    /// Light the arc from the center (for values that go both ways, like dB).
    pub bipolar: bool,
    pub unit: &'static str,
    pub decimals: usize,
}

impl KnobSpec {
    fn norm_of(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        if self.log && self.min > 0.0 {
            (v / self.min).ln() / (self.max / self.min).ln()
        } else {
            (v - self.min) / (self.max - self.min)
        }
    }
    fn value_at(&self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if self.log && self.min > 0.0 {
            self.min * (self.max / self.min).powf(t)
        } else {
            self.min + t * (self.max - self.min)
        }
    }
    pub fn format(&self, v: f32) -> String {
        let sign = if self.bipolar && v > 0.0 { "+" } else { "" };
        let num = if self.unit == "Hz" && v >= 1000.0 {
            format!("{:.1}k", v / 1000.0)
        } else {
            format!("{v:.prec$}", prec = self.decimals)
        };
        if self.unit.is_empty() { format!("{sign}{num}") } else { format!("{sign}{num} {}", self.unit) }
    }
}

/// Start and sweep of a knob's travel: from lower-left, clockwise, to lower-right.
const KNOB_START: f32 = std::f32::consts::PI * 0.75;
const KNOB_SWEEP: f32 = std::f32::consts::PI * 1.5;

/// A machined knob with an LED arc. Drag up/down to turn (Shift for fine),
/// double-click to reset. Once focused (click or Tab): arrow keys nudge,
/// Page Up/Down jump, Home/End go to the ends, Delete resets, and the mouse
/// wheel nudges. The value prints on a small
/// readout under the knob.
pub fn knob(ui: &mut Ui, value: &mut f32, spec: &KnobSpec, accent: Color32, diameter: f32) -> Response {
    let width = diameter.max(64.0);
    let size = Vec2::new(width, diameter + 36.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let c = Pos2::new(rect.center().x, rect.top() + diameter / 2.0 + 2.0);

    // Interaction.
    let mut t = spec.norm_of(*value);
    let before = t;
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        t -= resp.drag_delta().y / if fine { 900.0 } else { 180.0 };
    }
    // The wheel turns a knob only once it has focus (clicked or tabbed to),
    // and the knob then consumes the scroll. Otherwise scrolling a panel full
    // of knobs would turn every knob the pointer passed over.
    if resp.hovered() && resp.has_focus() {
        let scroll = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta.y));
        if scroll != 0.0 {
            t += scroll / 1500.0;
        }
    }
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    if resp.double_clicked() {
        t = spec.norm_of(spec.default);
    }
    // Keyboard: arrows nudge (Shift for fine), Page Up/Down jump, Home/End go
    // to the ends, Delete or Backspace resets. Arrow keys are claimed while
    // focused so they turn the knob instead of moving focus away.
    if resp.has_focus() {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() },
            )
        });
        let (up, down, pg_up, pg_down, home, end, reset, fine) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                    | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                    | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp)
                    | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                    | i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                    | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown)
                    | i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft),
                i.consume_key(egui::Modifiers::NONE, egui::Key::PageUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::PageDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Home),
                i.consume_key(egui::Modifiers::NONE, egui::Key::End),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    | i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace),
                i.modifiers.shift,
            )
        });
        let step = if fine { 0.002 } else { 0.01 };
        if up {
            t += step;
        }
        if down {
            t -= step;
        }
        if pg_up {
            t += 0.1;
        }
        if pg_down {
            t -= 0.1;
        }
        if home {
            t = 0.0;
        }
        if end {
            t = 1.0;
        }
        if reset {
            t = spec.norm_of(spec.default);
        }
    }
    if t != before {
        *value = spec.value_at(t);
        t = spec.norm_of(*value);
        resp.mark_changed();
    }
    let resp = resp.on_hover_cursor(CursorIcon::ResizeVertical);
    let label = spec.label;
    let readout_text = spec.format(*value);
    resp.widget_info(|| egui::WidgetInfo::slider(true, *value as f64, format!("{label}: {readout_text}")));
    let active = resp.dragged() || resp.hovered() || resp.has_focus();

    let painter = ui.painter();
    let r_arc = diameter / 2.0 - 1.0;
    let r_body = diameter * 0.36;

    // LED arc: dark track, then the lit part with a glow under it.
    painter.add(Shape::line(arc_points(c, r_arc, KNOB_START, KNOB_START + KNOB_SWEEP), Stroke::new(4.0, black(190))));
    let (a0, a1) = if spec.bipolar {
        let mid = KNOB_START + KNOB_SWEEP * 0.5;
        let a = KNOB_START + KNOB_SWEEP * t;
        (mid.min(a), mid.max(a))
    } else {
        (KNOB_START, KNOB_START + KNOB_SWEEP * t)
    };
    if a1 - a0 > 0.01 {
        let pts = arc_points(c, r_arc, a0, a1);
        let glow = if active { 1.0 } else { 0.7 };
        painter.add(Shape::line(pts.clone(), Stroke::new(9.0, accent.gamma_multiply(0.10 * glow))));
        painter.add(Shape::line(pts.clone(), Stroke::new(5.5, accent.gamma_multiply(0.25 * glow))));
        painter.add(Shape::line(pts, Stroke::new(2.5, theme::lerp(accent, Color32::WHITE, 0.25))));
    }
    // Scale ticks between the arc and the knob.
    for i in 0..=10 {
        let a = KNOB_START + KNOB_SWEEP * i as f32 / 10.0;
        let d = Vec2::new(a.cos(), a.sin());
        let (r0, r1) = if i % 5 == 0 { (r_body + 5.0, r_body + 10.0) } else { (r_body + 6.0, r_body + 8.5) };
        painter.line_segment([c + d * r0, c + d * r1], Stroke::new(1.0, SILK_DIM.gamma_multiply(0.8)));
    }

    if resp.has_focus() {
        painter.circle_stroke(c, r_arc + 5.0, Stroke::new(2.0, FOCUS));
    }

    // Body: shadow, knurled skirt, then a domed cap lit from above.
    round_shadow(painter, c, r_body, 3.5);
    painter.add(gradient_disc(c, r_body, Color32::from_rgb(70, 76, 84), Color32::from_rgb(24, 27, 31)));
    for i in 0..48 {
        let a = i as f32 / 48.0 * std::f32::consts::TAU;
        let d = Vec2::new(a.cos(), a.sin());
        let light = (-(a.sin()) * 0.5 + 0.5) * 60.0;
        let col = if i % 2 == 0 { white(light as u8 / 3 + 6) } else { black(70) };
        painter.line_segment([c + d * (r_body - 3.0), c + d * (r_body - 0.5)], Stroke::new(1.0, col));
    }
    let r_cap = r_body - 4.0;
    painter.add(gradient_disc(c, r_cap, Color32::from_rgb(92, 99, 108), Color32::from_rgb(38, 42, 48)));
    // Rim: highlight on the upper half, shadow on the lower half.
    painter.add(Shape::line(arc_points(c, r_cap, std::f32::consts::PI * 1.1, std::f32::consts::PI * 1.9), Stroke::new(1.0, white(70))));
    painter.add(Shape::line(arc_points(c, r_cap, std::f32::consts::PI * 0.15, std::f32::consts::PI * 0.85), Stroke::new(1.0, black(110))));
    // Pointer.
    let a = KNOB_START + KNOB_SWEEP * t;
    let d = Vec2::new(a.cos(), a.sin());
    painter.line_segment([c + d * (r_cap * 0.3), c + d * (r_cap * 0.92)], Stroke::new(2.6, black(120)));
    painter.line_segment([c + d * (r_cap * 0.3), c + d * (r_cap * 0.9)], Stroke::new(2.0, Color32::from_rgb(232, 240, 244)));
    if active {
        led(painter, c + d * (r_cap * 0.62), 1.8, accent, true);
    }

    // Label and value readout.
    let label_y = c.y + diameter / 2.0 + 6.0;
    silk_text(painter, Pos2::new(c.x, label_y), Align2::CENTER_TOP, &spec.label.to_uppercase(), 11.5, SILK_DIM);
    let readout = Rect::from_center_size(Pos2::new(c.x, label_y + 22.0), Vec2::new(width - 6.0, 15.0));
    painter.rect_filled(readout.expand(1.0), CornerRadius::same(3), black(170));
    painter.rect_filled(readout, CornerRadius::same(3), GLASS);
    painter.text(
        readout.center(),
        Align2::CENTER_CENTER,
        spec.format(*value),
        FontId::monospace(11.0),
        if active { READOUT } else { READOUT.gamma_multiply(0.75) },
    );
    resp
}

// ------------------------------------------------------------------ button

/// What an LED button means, for screen readers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LedRole {
    /// Does something when pressed (load a preset, start calibration).
    Action,
    /// Switches a setting on and off.
    Toggle,
    /// One of a group of mutually exclusive options.
    Option,
}

/// A raised push button with an LED. `on` lights the LED and shows the button
/// latched in. Returns the click response.
pub fn led_button(ui: &mut Ui, label: &str, on: bool, accent: Color32, width: f32) -> Response {
    led_button_as(ui, label, on, accent, width, LedRole::Action)
}

/// A toggle switch drawn as an LED button.
pub fn led_toggle(ui: &mut Ui, label: &str, on: bool, accent: Color32, width: f32) -> Response {
    led_button_as(ui, label, on, accent, width, LedRole::Toggle)
}

/// One option of a group, drawn as an LED button.
pub fn led_option(ui: &mut Ui, label: &str, on: bool, accent: Color32, width: f32) -> Response {
    led_button_as(ui, label, on, accent, width, LedRole::Option)
}

/// A toggle whose printed label is short ("ON") but whose spoken name says
/// what it switches ("High-pass on").
pub fn led_toggle_named(ui: &mut Ui, label: &str, name: &str, on: bool, accent: Color32, width: f32) -> Response {
    led_button_full(ui, label, name, on, accent, width, LedRole::Toggle)
}

pub fn led_button_as(ui: &mut Ui, label: &str, on: bool, accent: Color32, width: f32, role: LedRole) -> Response {
    led_button_full(ui, label, label, on, accent, width, role)
}

fn led_button_full(
    ui: &mut Ui,
    label: &str,
    name: &str,
    on: bool,
    accent: Color32,
    width: f32,
    role: LedRole,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 28.0), Sense::click());
    resp.widget_info(|| match role {
        LedRole::Action => egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name),
        LedRole::Toggle => egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, name),
        LedRole::Option => egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, name),
    });
    let painter = ui.painter();
    let pressed = resp.is_pointer_button_down_on() || on;
    let r = CornerRadius::same(4);
    // Well the button sits in.
    painter.rect_filled(rect.expand(1.5), CornerRadius::same(5), black(170));
    let cap = if pressed { rect.translate(Vec2::new(0.0, 1.0)).shrink2(Vec2::new(0.0, 0.5)) } else { rect };
    if !pressed {
        painter.add(Shape::from(Shadow { offset: [0, 2], blur: 4, spread: 0, color: black(120) }.as_shape(rect, r)));
    }
    let (top, bottom) = if pressed {
        (Color32::from_rgb(40, 44, 50), Color32::from_rgb(52, 57, 64))
    } else if resp.hovered() {
        (Color32::from_rgb(76, 83, 92), Color32::from_rgb(46, 50, 56))
    } else {
        (Color32::from_rgb(66, 72, 80), Color32::from_rgb(40, 44, 49))
    };
    painter.add(gradient_rect(cap, top, bottom));
    if !pressed {
        painter.line_segment([cap.left_top() + Vec2::new(3.0, 0.5), cap.right_top() + Vec2::new(-3.0, 0.5)], Stroke::new(1.0, white(60)));
    }
    led(painter, Pos2::new(cap.left() + 12.0, cap.center().y), 3.2, accent, on);
    let text_col = if on { theme::lerp(SILK, accent, 0.35) } else { SILK };
    silk_text(painter, Pos2::new(cap.left() + 23.0, cap.center().y), Align2::LEFT_CENTER, label, 12.0, text_col);
    if resp.has_focus() {
        painter.rect_stroke(rect.expand(3.0), CornerRadius::same(6), Stroke::new(2.0, FOCUS), egui::StrokeKind::Outside);
    }
    resp
}

// ------------------------------------------------------------------ meters

/// A vertical LED ladder in a glass window, with peak hold. `frac` is 0..1.
pub fn led_ladder(ui: &mut Ui, id: Id, label: &str, frac: f32, size: Vec2, help: &str) -> Response {
    let frac = frac.clamp(0.0, 1.0);
    // Peak hold: hold for a second, then fall.
    let now = ui.input(|i| i.time);
    let (mut peak, mut at) = ui.data(|d| d.get_temp::<(f32, f64)>(id)).unwrap_or((0.0, now));
    if frac >= peak {
        peak = frac;
        at = now;
    } else if now - at > 1.0 {
        peak = (peak - 0.012).max(frac);
    }
    ui.data_mut(|d| d.insert_temp(id, (peak, at)));

    let total = Vec2::new(size.x.max(30.0), size.y + 20.0);
    let (outer, resp) = ui.allocate_exact_size(total, Sense::hover());
    let rect = Rect::from_min_size(Pos2::new(outer.center().x - size.x / 2.0, outer.top()), size);
    let resp = resp.on_hover_text(help);
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::ProgressIndicator,
            true,
            format!("{label} meter: {:.0} percent", frac * 100.0),
        )
    });
    let painter = ui.painter();
    painter.rect_filled(rect.expand(2.0), CornerRadius::same(4), black(170));
    painter.rect_filled(rect, CornerRadius::same(3), GLASS);

    let segs = ((size.y - 6.0) / 6.0).floor().max(4.0) as usize;
    let gap = 2.0;
    let seg_h = (size.y - 6.0 - gap * (segs as f32 - 1.0)) / segs as f32;
    let lit = (frac * segs as f32).ceil() as usize;
    let peak_seg = ((peak * segs as f32).ceil() as usize).saturating_sub(1);
    for i in 0..segs {
        let y = rect.bottom() - 3.0 - (i + 1) as f32 * seg_h - i as f32 * gap;
        let sr = Rect::from_min_size(Pos2::new(rect.left() + 4.0, y), Vec2::new(rect.width() - 8.0, seg_h));
        let t = i as f32 / (segs - 1) as f32;
        let col = if t > 0.85 {
            Color32::from_rgb(255, 72, 56)
        } else {
            theme::lerp(theme::CYAN, theme::EMBER, ((t - 0.45) / 0.4).clamp(0.0, 1.0))
        };
        let on = i < lit || (peak > 0.02 && i == peak_seg);
        if on {
            painter.rect_filled(sr.expand(1.5), CornerRadius::same(2), col.gamma_multiply(0.18));
            painter.rect_filled(sr, CornerRadius::same(1), col);
        } else {
            painter.rect_filled(sr, CornerRadius::same(1), theme::lerp(GLASS, col, 0.12));
        }
    }
    let gloss = Rect::from_min_max(rect.min + Vec2::new(1.0, 1.0), Pos2::new(rect.center().x, rect.bottom() - 1.0));
    painter.rect_filled(gloss, CornerRadius::same(2), white(6));
    silk_text(painter, Pos2::new(outer.center().x, rect.bottom() + 6.0), Align2::CENTER_TOP, label, 11.0, SILK_DIM);
    resp
}

/// The metal strip behind the app header: same finish as a faceplate, with a
/// bevel along its bottom edge instead of a border.
pub fn header_strip(rect: Rect) -> Shape {
    let mut shapes = vec![gradient_rect(rect, Color32::from_rgb(44, 49, 56), Color32::from_rgb(24, 27, 31))];
    let mut y = rect.top() + 1.0;
    let mut seed = 0x9E37_79B9u32;
    while y < rect.bottom() - 1.0 {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let v = (seed >> 24) as u8;
        let col = if v & 1 == 0 { white(v % 6) } else { black(v % 10) };
        shapes.push(Shape::line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, col)));
        y += 1.0 + (v % 3) as f32;
    }
    shapes.push(Shape::line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, black(220))));
    shapes.push(Shape::line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0, white(30))));
    Shape::Vec(shapes)
}

/// A node in the graph editor drawn as a small rack module: metal plate,
/// bevel, two screws, an accent nameplate strip and a status LED (lit when the
/// node is active). `zoom` scales every detail with the canvas.
pub fn module_box(painter: &egui::Painter, rect: Rect, zoom: f32, accent: Color32, selected: bool, hovered: bool, bypass: bool) {
    let r = CornerRadius::same((5.0 * zoom) as u8);
    let shadow = Shadow { offset: [0, (5.0 * zoom) as i8], blur: (14.0 * zoom) as u8, spread: 0, color: black(160) };
    painter.add(Shape::from(shadow.as_shape(rect, r)));
    if selected {
        painter.rect_filled(rect.expand(3.0 * zoom), CornerRadius::same((7.0 * zoom) as u8), accent.gamma_multiply(0.25));
    }
    painter.rect_filled(rect.expand(1.0), r, black(220));
    let (top, bottom) = if hovered || selected { (Color32::from_rgb(54, 60, 68), Color32::from_rgb(32, 36, 41)) } else { (METAL_TOP, METAL_BOTTOM) };
    painter.add(gradient_rect(rect.shrink(0.5), top, bottom));
    painter.line_segment([rect.left_top() + Vec2::new(3.0, 0.5), rect.right_top() + Vec2::new(-3.0, 0.5)], Stroke::new(1.0, white(55)));
    painter.line_segment([rect.left_bottom() + Vec2::new(3.0, -0.5), rect.right_bottom() + Vec2::new(-3.0, -0.5)], Stroke::new(1.0, black(150)));
    // Nameplate strip along the top, lit in the accent unless bypassed.
    let strip = Rect::from_min_size(rect.min + Vec2::new(6.0 * zoom, 5.0 * zoom), Vec2::new(rect.width() - 12.0 * zoom, 3.0 * zoom));
    painter.rect_filled(strip, CornerRadius::same(1), if bypass { theme::lerp(GLASS, accent, 0.2) } else { accent });
    if !bypass {
        painter.rect_filled(strip.expand(2.0 * zoom), CornerRadius::same(2), accent.gamma_multiply(0.12));
    }
    if zoom > 0.6 {
        for (i, c) in [rect.left_bottom() + Vec2::new(7.0, -7.0) * zoom, rect.right_bottom() + Vec2::new(-7.0, -7.0) * zoom].into_iter().enumerate() {
            screw(painter, c, 0.7 + i as f32 * 1.3);
        }
    }
    led(painter, rect.right_top() + Vec2::new(-12.0, 18.0) * zoom, 3.0 * zoom, if bypass { theme::EMBER } else { theme::GOOD }, true);
}

/// A patch jack: a dark socket in a machined collar. `color` tints the collar
/// so inputs and outputs read differently.
pub fn jack(painter: &egui::Painter, c: Pos2, zoom: f32, color: Color32) {
    let r = 7.5 * zoom;
    painter.circle_filled(c + Vec2::new(0.0, 1.5 * zoom), r + 1.0, black(140));
    painter.add(gradient_disc(c, r, theme::lerp(Color32::from_rgb(150, 156, 162), color, 0.45), Color32::from_rgb(40, 44, 48)));
    painter.circle_filled(c, r * 0.55, Color32::from_rgb(6, 7, 8));
    painter.circle_stroke(c, r * 0.55, Stroke::new(1.0, black(200)));
}

/// The dark rack the panels are mounted in: a deep vertical falloff.
pub fn rack_backdrop(painter: &egui::Painter, rect: Rect) {
    painter.add(gradient_rect(rect, Color32::from_rgb(19, 22, 26), Color32::from_rgb(8, 9, 11)));
}

/// A small amber display showing `text`, recessed like the knob readouts.
pub fn readout(ui: &mut Ui, text: &str, width: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 20.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    let painter = ui.painter();
    painter.rect_filled(rect.expand(1.5), CornerRadius::same(4), black(180));
    painter.rect_filled(rect, CornerRadius::same(3), GLASS);
    painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::monospace(12.0), READOUT);
    let gloss = Rect::from_min_max(rect.min + Vec2::new(1.0, 1.0), Pos2::new(rect.right() - 1.0, rect.center().y));
    painter.add(gradient_rect(gloss, white(10), white(0)));
    resp
}

/// Text in the silkscreen style, laid out as a normal widget.
pub fn silk_label(ui: &mut Ui, text: &str, size: f32) -> Response {
    // Never smaller than 11.5 px: this is the faceplate's smallest print.
    let size = size.max(11.5);
    let galley_w = text.len() as f32 * size * 0.62;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley_w, size + 4.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
    silk_text(ui.painter(), rect.left_center(), Align2::LEFT_CENTER, text, size, SILK_DIM);
    resp
}
