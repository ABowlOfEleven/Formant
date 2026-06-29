//! The themed eframe UI: tabbed Mixer / Nodes / Presets / Settings.
//!
//! The Nodes tab is a hand-built drag-wire graph editor (canvas painted with the
//! `Painter`, interaction via `ui.interact`), with a side inspector for the
//! selected node's parameters.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2};

use formant_audio::devices::{self, Direction};
use formant_audio::{hotkeys, Action};
use formant_core::{Config, Graph, MuteMode, NodeId, NodeKind, NodeParams, Preset};

use crate::engine::Engine;
use crate::theme;

const NODE_SIZE: Vec2 = Vec2::new(150.0, 62.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mixer,
    Nodes,
    Presets,
    Settings,
}

pub struct FormantApp {
    _com: formant_audio::com::ComGuard,
    engine: Engine,
    config: Config,
    graph: Graph,
    last_pushed: Graph,
    tab: Tab,
    presets: Vec<Preset>,
    new_preset_name: String,
    rebinding: Option<Action>,
    capture_devices: Vec<String>,
    render_devices: Vec<String>,
    status: String,
    // Node editor state.
    selected: Option<NodeId>,
    dragging_from: Option<NodeId>,
    pan: Vec2,
    // VST3 state.
    plugins: Vec<formant_vst3::Plugin>,
    installed_vsts: HashSet<NodeId>,
    node_params: HashMap<NodeId, Vec<VstParam>>,
    editors: HashMap<NodeId, formant_vst3::PluginEditor>,
}

/// A VST node parameter as shown in the inspector (normalized value).
struct VstParam {
    id: u32,
    name: String,
    value: f32,
}

impl FormantApp {
    pub fn new(config: Config, engine: Engine, com: formant_audio::com::ComGuard, graph: Graph) -> Self {
        Self {
            _com: com,
            engine,
            config,
            graph: graph.clone(),
            last_pushed: graph,
            tab: Tab::Mixer,
            presets: Preset::load_all(),
            new_preset_name: String::new(),
            rebinding: None,
            capture_devices: Vec::new(),
            render_devices: Vec::new(),
            status: String::new(),
            selected: None,
            dragging_from: None,
            pan: Vec2::ZERO,
            plugins: Vec::new(),
            installed_vsts: HashSet::new(),
            node_params: HashMap::new(),
            editors: HashMap::new(),
        }
    }

    /// Pull parameter edits made in plugin GUIs and apply them to the processor,
    /// the sliders, and the persisted node state.
    fn drain_gui_edits(&mut self) {
        let mut edits: Vec<(NodeId, u32, f64)> = Vec::new();
        for (nid, editor) in &self.editors {
            for (pid, val) in editor.take_edits() {
                edits.push((*nid, pid, val));
            }
        }
        for (nid, pid, val) in edits {
            self.engine.set_effect_param(nid, pid, val);
            if let Some(plist) = self.node_params.get_mut(&nid) {
                if let Some(p) = plist.iter_mut().find(|p| p.id == pid) {
                    p.value = val as f32;
                }
            }
            if let Some(node) = self.graph.node_mut(nid) {
                if let NodeParams::Vst3 { params, .. } = &mut node.params {
                    upsert_param(params, pid, val);
                }
            }
        }
    }

    /// Instantiate/drop VST plugin instances so the audio thread's effects match
    /// the `Vst3` nodes in the graph. Instantiation (slow) happens here on the UI
    /// thread; the live instance is handed to the audio thread via the engine.
    fn reconcile_vsts(&mut self) {
        let want: Vec<(NodeId, String, Vec<(u32, f64)>)> = self
            .graph
            .nodes
            .iter()
            .filter_map(|n| match &n.params {
                NodeParams::Vst3 { binary, params, .. } if !binary.is_empty() => {
                    Some((n.id, binary.clone(), params.clone()))
                }
                _ => None,
            })
            .collect();
        let want_ids: HashSet<NodeId> = want.iter().map(|(id, ..)| *id).collect();

        let stale: Vec<NodeId> = self
            .installed_vsts
            .iter()
            .copied()
            .filter(|id| !want_ids.contains(id))
            .collect();
        for id in stale {
            self.engine.remove_effect(id);
            self.installed_vsts.remove(&id);
            self.node_params.remove(&id);
            self.editors.remove(&id); // drops the editor -> closes its window
        }

        for (id, binary, stored) in want {
            if self.installed_vsts.contains(&id) {
                continue;
            }
            match crate::vst::VstEffect::load(Path::new(&binary)) {
                Ok((eff, editor)) => {
                    // Display list, seeded with persisted values where present.
                    let params = editor
                        .params()
                        .iter()
                        .map(|d| {
                            let value = stored
                                .iter()
                                .find(|(pid, _)| *pid == d.id)
                                .map(|(_, v)| *v as f32)
                                .unwrap_or(d.default as f32);
                            VstParam { id: d.id, name: d.name.clone(), value }
                        })
                        .collect();
                    self.node_params.insert(id, params);
                    self.engine.install_effect(id, Box::new(eff));
                    // Re-apply persisted values to both controller and processor.
                    for &(pid, val) in &stored {
                        editor.set_param(pid, val);
                        self.engine.set_effect_param(id, pid, val);
                    }
                    self.editors.insert(id, editor);
                    self.status = "VST node loaded".into();
                }
                Err(e) => self.status = format!("VST load failed: {e}"),
            }
            self.installed_vsts.insert(id); // mark either way to avoid retry spam
        }
    }

    fn save_config(&mut self) {
        self.config.bindings = self.engine.bindings.snapshot();
        match self.config.save() {
            Ok(()) => self.status = "config saved".into(),
            Err(e) => self.status = format!("config save failed: {e}"),
        }
    }

    fn restart_engine(&mut self) {
        self.config.bindings = self.engine.bindings.snapshot();
        self.engine.stop();
        match Engine::start(&self.config, self.graph.clone()) {
            Ok(e) => {
                self.engine = e;
                self.last_pushed = self.graph.clone();
                self.installed_vsts.clear(); // fresh engine: re-install VST nodes
                self.editors.clear();
                self.status = "engine restarted with new devices".into();
            }
            Err(e) => self.status = format!("restart failed: {e}"),
        }
    }

    fn refresh_devices(&mut self) {
        self.capture_devices = devices::list(Direction::Capture)
            .map(|v| v.into_iter().map(|d| d.name).collect())
            .unwrap_or_default();
        self.render_devices = devices::list(Direction::Render)
            .map(|v| v.into_iter().map(|d| d.name).collect())
            .unwrap_or_default();
    }
}

impl eframe::App for FormantApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint(); // live meters

        if let Some(action) = self.rebinding {
            if let Some(vk) = hotkeys::first_pressed_key() {
                if vk == 0x1B {
                    self.status = "rebind cancelled".into();
                } else {
                    self.engine.bindings.set(action, Some(vk));
                    self.status = format!("{} bound to {}", action.label(), hotkeys::key_name(vk));
                    self.save_config();
                }
                self.rebinding = None;
            }
        }

        self.top_bar(ui);
        egui::CentralPanel::default().show_inside(ui, |ui| match self.tab {
            Tab::Mixer => self.tab_mixer(ui),
            Tab::Nodes => self.tab_nodes(ui),
            Tab::Presets => self.tab_presets(ui),
            Tab::Settings => self.tab_settings(ui),
        });

        if self.graph != self.last_pushed {
            self.engine.push_graph(&self.graph);
            self.last_pushed = self.graph.clone();
        }
        self.reconcile_vsts();
        self.drain_gui_edits();
    }
}

impl FormantApp {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("FORMANT").color(theme::CYAN).strong().size(20.0));
                ui.label(RichText::new("vocal processor").color(theme::MUTED).small());
                ui.separator();
                for (tab, name) in [
                    (Tab::Mixer, "Mixer"),
                    (Tab::Nodes, "Nodes"),
                    (Tab::Presets, "Presets"),
                    (Tab::Settings, "Settings"),
                ] {
                    if ui.selectable_label(self.tab == tab, name).clicked() {
                        self.tab = tab;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let bypassed = self.engine.controls.global_bypass();
                    let (txt, col) = if bypassed {
                        ("● BYPASSED", theme::EMBER)
                    } else {
                        ("○ live", theme::GOOD)
                    };
                    if ui.button(RichText::new(txt).color(col)).clicked() {
                        self.engine.controls.toggle_bypass();
                    }
                });
            });
            ui.add_space(4.0);
        });
    }

    fn tab_mixer(&mut self, ui: &mut egui::Ui) {
        let (in_peak, out_peak, vad, gr) = {
            let m = &self.engine.meters;
            (m.in_peak(), m.out_peak(), m.vad(), m.gain_reduction_db())
        };
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("METERS").color(theme::CYAN).strong());
            meter(ui, "input", in_peak, theme::CYAN);
            meter(ui, "output", out_peak, theme::GOOD);
            meter(ui, "voice (VAD)", vad, theme::EMBER);
            meter(ui, "comp GR", (gr / 20.0).clamp(0.0, 1.0), theme::EMBER);
        });

        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("MUTE MODE").color(theme::CYAN).strong());
            ui.horizontal(|ui| {
                let cur = self.engine.controls.mode();
                for m in [MuteMode::Vad, MuteMode::PushToTalk, MuteMode::Toggle, MuteMode::AlwaysOpen] {
                    if ui.selectable_label(cur == m, m.label()).clicked() {
                        self.engine.controls.set_mode(m);
                    }
                }
            });
        });

        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("CHAIN").color(theme::CYAN).strong());
            let ids: Vec<NodeId> = self
                .graph
                .nodes
                .iter()
                .filter(|n| !matches!(n.params.kind(), NodeKind::Input | NodeKind::Output))
                .map(|n| n.id)
                .collect();
            ui.horizontal_wrapped(|ui| {
                for id in ids {
                    if let Some(n) = self.graph.node(id).cloned() {
                        let mut on = !n.bypass;
                        if ui.checkbox(&mut on, node_label(&n.params)).changed() {
                            if let Some(nm) = self.graph.node_mut(id) {
                                nm.bypass = !on;
                            }
                        }
                    }
                }
            });
        });
    }

    fn tab_nodes(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_top(|ui| {
            let canvas_size = Vec2::new((ui.available_width() - 236.0).max(200.0), ui.available_height());
            let (canvas, bg) = ui.allocate_exact_size(canvas_size, Sense::click_and_drag());
            self.draw_canvas(ui, canvas, bg);

            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(224.0);
                self.draw_inspector(ui);
            });
        });
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui, canvas: Rect, bg: egui::Response) {
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, CornerRadius::same(4), theme::BG);

        if self.dragging_from.is_none() && bg.dragged() {
            self.pan += bg.drag_delta();
        }
        if bg.clicked() {
            self.selected = None;
        }

        let origin = canvas.min.to_vec2() + self.pan;
        // Snapshot positions/kinds so we don't borrow the graph while mutating it.
        let layout: HashMap<NodeId, (Pos2, NodeKind, bool)> = self
            .graph
            .nodes
            .iter()
            .map(|n| (n.id, (Pos2::new(origin.x + n.pos[0], origin.y + n.pos[1]), n.params.kind(), n.bypass)))
            .collect();
        let conns = self.graph.connections.clone();
        let ids: Vec<NodeId> = self.graph.nodes.iter().map(|n| n.id).collect();
        // Display labels (VST nodes show the plugin name).
        let labels: HashMap<NodeId, String> = self
            .graph
            .nodes
            .iter()
            .map(|n| (n.id, node_label(&n.params)))
            .collect();

        // Wires (under nodes).
        for c in &conns {
            if let (Some(&(fp, ..)), Some(&(tp, ..))) = (layout.get(&c.from), layout.get(&c.to)) {
                wire(&painter, fp + Vec2::new(NODE_SIZE.x, NODE_SIZE.y * 0.5), tp + Vec2::new(0.0, NODE_SIZE.y * 0.5));
            }
        }
        // Pending wire while dragging from an output port.
        if let Some(fid) = self.dragging_from {
            if let (Some(&(fp, ..)), Some(ptr)) = (layout.get(&fid), ui.input(|i| i.pointer.interact_pos())) {
                let out = fp + Vec2::new(NODE_SIZE.x, NODE_SIZE.y * 0.5);
                painter.line_segment([out, ptr], Stroke::new(2.0, theme::EMBER));
            }
        }

        // Nodes.
        for id in &ids {
            let id = *id;
            let &(pos, kind, bypass) = layout.get(&id).unwrap();
            let rect = Rect::from_min_size(pos, NODE_SIZE);

            let resp = ui.interact(rect, egui::Id::new(("fmt_node", id)), Sense::click_and_drag());
            if resp.dragged() {
                if let Some(n) = self.graph.node_mut(id) {
                    let d = resp.drag_delta();
                    n.pos[0] += d.x;
                    n.pos[1] += d.y;
                }
            }
            if resp.clicked() {
                self.selected = Some(id);
            }

            let selected = self.selected == Some(id);
            let border = if selected { theme::CYAN } else { theme::CYAN.gamma_multiply(0.4) };
            painter.rect_filled(rect, CornerRadius::same(6), theme::CARD);
            painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(if selected { 2.0 } else { 1.0 }, border), StrokeKind::Inside);
            let title_col = if bypass { theme::MUTED } else { theme::CYAN };
            let label = labels.get(&id).cloned().unwrap_or_else(|| kind.label().to_string());
            painter.text(rect.min + Vec2::new(12.0, 10.0), Align2::LEFT_TOP, label, FontId::proportional(15.0), title_col);
            if kind == NodeKind::Vst3 {
                painter.text(rect.min + Vec2::new(12.0, 32.0), Align2::LEFT_TOP, "vst3", FontId::proportional(10.0), theme::EMBER);
            }
            if bypass {
                painter.text(rect.min + Vec2::new(12.0, 36.0), Align2::LEFT_TOP, "bypassed", FontId::proportional(11.0), theme::EMBER);
            }

            // Input port (left) — click to disconnect.
            if kind != NodeKind::Input {
                let p = rect.left_center();
                painter.circle_filled(p, 5.0, theme::EMBER);
                if ui.interact(Rect::from_center_size(p, Vec2::splat(16.0)), egui::Id::new(("fmt_in", id)), Sense::click()).clicked() {
                    self.graph.disconnect_into(id);
                }
            }
            // Output port (right) — drag to connect.
            if kind != NodeKind::Output {
                let p = rect.right_center();
                painter.circle_filled(p, 5.0, theme::CYAN);
                if ui.interact(Rect::from_center_size(p, Vec2::splat(16.0)), egui::Id::new(("fmt_out", id)), Sense::drag()).drag_started() {
                    self.dragging_from = Some(id);
                }
            }
        }

        // Finalize a connection on pointer release.
        if self.dragging_from.is_some() && ui.input(|i| i.pointer.any_released()) {
            let from = self.dragging_from.take().unwrap();
            if let Some(ptr) = ui.input(|i| i.pointer.interact_pos()) {
                let target = layout
                    .iter()
                    .filter(|(_, (_, kind, _))| *kind != NodeKind::Input)
                    .find(|(_, (pos, ..))| {
                        Rect::from_center_size(Rect::from_min_size(*pos, NODE_SIZE).left_center(), Vec2::splat(18.0)).contains(ptr)
                    })
                    .map(|(id, _)| *id);
                if let Some(to) = target {
                    self.graph.connect(from, to);
                }
            }
        }
    }

    fn draw_inspector(&mut self, ui: &mut egui::Ui) {
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("ADD NODE").color(theme::CYAN).strong());
            ui.horizontal_wrapped(|ui| {
                for kind in NodeKind::EFFECTS {
                    if ui.button(kind.label()).clicked() {
                        let pos = [-self.pan.x + 220.0, -self.pan.y + 220.0];
                        let id = self.graph.add_node(kind.default_params(), pos);
                        self.selected = Some(id);
                    }
                }
            });
            if ui.button("Reset to default chain").clicked() {
                self.graph = Graph::default_chain();
                self.selected = None;
                self.pan = Vec2::ZERO;
            }
        });

        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("VST3 PLUGINS").color(theme::CYAN).strong());
                if ui.button("Scan").clicked() {
                    self.plugins = formant_vst3::scan();
                    self.status = format!("found {} plugins", self.plugins.len());
                }
            });
            if self.plugins.is_empty() {
                ui.label(RichText::new("press Scan to list installed plugins").color(theme::MUTED).small());
            }
            let effects: Vec<(String, String)> = self
                .plugins
                .iter()
                .filter(|p| p.is_effect())
                .map(|p| (p.name.clone(), p.binary.to_string_lossy().into_owned()))
                .collect();
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for (name, binary) in effects {
                    ui.horizontal(|ui| {
                        if ui.button("+").clicked() {
                            let pos = [-self.pan.x + 250.0, -self.pan.y + 280.0];
                            let id = self.graph.add_node(
                                NodeParams::Vst3 { binary, name: name.clone(), params: Vec::new() },
                                pos,
                            );
                            self.selected = Some(id);
                        }
                        ui.label(RichText::new(&name).small());
                    });
                }
            });
        });

        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("INSPECTOR").color(theme::CYAN).strong());
            let Some(id) = self.selected else {
                ui.label(RichText::new("select a node").color(theme::MUTED).small());
                return;
            };
            let Some(node) = self.graph.node(id).cloned() else {
                self.selected = None;
                return;
            };

            ui.label(RichText::new(node_label(&node.params)).color(theme::EMBER));
            let mut params = node.params;
            let mut on = !node.bypass;
            ui.checkbox(&mut on, "enabled");

            match &mut params {
                NodeParams::HighPass { cutoff_hz } => {
                    ui.add(egui::Slider::new(cutoff_hz, 20.0..=400.0).text("cutoff Hz"));
                }
                NodeParams::Gate { threshold, vad_gate } => {
                    ui.add(egui::Slider::new(threshold, 0.0..=0.2).text("threshold"));
                    ui.checkbox(vad_gate, "follow VAD");
                }
                NodeParams::DeEsser { threshold_db, ratio } => {
                    ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("thr dB"));
                    ui.add(egui::Slider::new(ratio, 1.0..=12.0).text("ratio"));
                }
                NodeParams::Compressor { threshold_db, ratio } => {
                    ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("thr dB"));
                    ui.add(egui::Slider::new(ratio, 1.0..=20.0).text("ratio"));
                }
                NodeParams::Eq { low_db, mid_db, high_db } => {
                    ui.add(egui::Slider::new(low_db, -12.0..=12.0).text("low"));
                    ui.add(egui::Slider::new(mid_db, -12.0..=12.0).text("mid"));
                    ui.add(egui::Slider::new(high_db, -12.0..=12.0).text("high"));
                }
                NodeParams::Makeup { gain_db } => {
                    ui.add(egui::Slider::new(gain_db, -24.0..=24.0).text("gain dB"));
                }
                NodeParams::Vst3 { name, params: stored, .. } => {
                    ui.label(RichText::new(name.as_str()).color(theme::MUTED));
                    if self.editors.contains_key(&id)
                        && ui.button(RichText::new("Open plugin editor").color(theme::CYAN)).clicked()
                    {
                        if let Some(ed) = self.editors.get_mut(&id) {
                            if let Err(e) = ed.open() {
                                self.status = format!("editor: {e}");
                            }
                        }
                    }
                    match self.node_params.get_mut(&id) {
                        Some(plist) if !plist.is_empty() => {
                            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                                for p in plist.iter_mut() {
                                    if ui.add(egui::Slider::new(&mut p.value, 0.0..=1.0).text(&p.name)).changed() {
                                        let v = p.value as f64;
                                        self.engine.set_effect_param(id, p.id, v);
                                        if let Some(ed) = self.editors.get(&id) {
                                            ed.set_param(p.id, v);
                                        }
                                        upsert_param(stored, p.id, v);
                                    }
                                }
                            });
                        }
                        Some(_) => {
                            ui.label(RichText::new("no editable parameters").color(theme::MUTED).small());
                        }
                        None => {
                            ui.label(RichText::new("loading…").color(theme::MUTED).small());
                        }
                    }
                }
                NodeParams::Denoise | NodeParams::Input | NodeParams::Output => {
                    ui.label(RichText::new("no parameters").color(theme::MUTED).small());
                }
            }

            let kind = params.kind();
            if let Some(n) = self.graph.node_mut(id) {
                n.params = params;
                n.bypass = !on;
            }

            if !matches!(kind, NodeKind::Input | NodeKind::Output) {
                ui.add_space(6.0);
                if ui.button(RichText::new("Delete node").color(theme::EMBER)).clicked() {
                    self.graph.remove_node(id);
                    self.selected = None;
                }
            }
        });

        if !self.status.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new(&self.status).color(theme::MUTED).small());
        }
    }

    fn tab_presets(&mut self, ui: &mut egui::Ui) {
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("SAVE CURRENT GRAPH").color(theme::CYAN).strong());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_preset_name).hint_text("preset name"));
                if ui.button("Save").clicked() && !self.new_preset_name.trim().is_empty() {
                    let preset = Preset::new(self.new_preset_name.trim(), self.graph.clone());
                    match preset.save() {
                        Ok(_) => {
                            self.status = format!("saved '{}'", preset.name);
                            self.new_preset_name.clear();
                            self.presets = Preset::load_all();
                        }
                        Err(e) => self.status = format!("save failed: {e}"),
                    }
                }
            });
        });

        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("PRESET CHAINS").color(theme::CYAN).strong());
                if ui.button("Refresh").clicked() {
                    self.presets = Preset::load_all();
                }
            });
            if self.presets.is_empty() {
                ui.label(RichText::new("no presets yet").color(theme::MUTED).small());
            }
            let mut to_delete: Option<usize> = None;
            for (i, preset) in self.presets.clone().into_iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&preset.name);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Delete").clicked() {
                            let _ = preset.delete();
                            to_delete = Some(i);
                        }
                        if ui.button("Load").clicked() {
                            self.graph = preset.graph.clone();
                            self.selected = None;
                            self.status = format!("loaded '{}'", preset.name);
                        }
                    });
                });
            }
            if let Some(i) = to_delete {
                self.presets.remove(i);
            }
        });
    }

    fn tab_settings(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            theme::card(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("DEVICES (name match)").color(theme::CYAN).strong());
                ui.label(RichText::new(format!("mic now: {}", self.engine.mic_name)).color(theme::MUTED).small());
                ui.horizontal(|ui| {
                    ui.label("mic");
                    ui.add(egui::TextEdit::singleline(&mut self.config.devices.mic));
                });
                let mut remove: Option<usize> = None;
                for i in 0..self.config.devices.outputs.len() {
                    ui.horizontal(|ui| {
                        ui.label("out");
                        ui.add(egui::TextEdit::singleline(&mut self.config.devices.outputs[i]));
                        if ui.button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.config.devices.outputs.remove(i);
                }
                ui.horizontal(|ui| {
                    if ui.button("+ output").clicked() {
                        self.config.devices.outputs.push(String::new());
                    }
                    if ui.button("Apply & restart").clicked() {
                        self.restart_engine();
                    }
                    if ui.button("List devices").clicked() {
                        self.refresh_devices();
                    }
                });
                if !self.capture_devices.is_empty() || !self.render_devices.is_empty() {
                    ui.collapsing("available devices", |ui| {
                        ui.label(RichText::new("capture:").color(theme::EMBER).small());
                        for d in &self.capture_devices {
                            ui.label(RichText::new(d).small());
                        }
                        ui.label(RichText::new("render:").color(theme::EMBER).small());
                        for d in &self.render_devices {
                            ui.label(RichText::new(d).small());
                        }
                    });
                }
            });

            ui.add_space(8.0);
            theme::card(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("HOTKEYS").color(theme::CYAN).strong());
                    if ui.button("Reset defaults").clicked() {
                        let d = formant_core::Bindings::default();
                        self.engine.bindings.set(Action::Ptt, d.ptt);
                        self.engine.bindings.set(Action::ToggleMute, d.toggle_mute);
                        self.engine.bindings.set(Action::Bypass, d.bypass);
                        self.engine.bindings.set(Action::CycleMode, d.cycle_mode);
                        self.save_config();
                    }
                });
                for action in Action::ALL {
                    ui.horizontal(|ui| {
                        ui.add_sized([150.0, 18.0], egui::Label::new(action.label()));
                        if self.rebinding == Some(action) {
                            ui.colored_label(theme::EMBER, "press a key…  (Esc cancels)");
                        } else {
                            let name = self.engine.bindings.get(action).map(hotkeys::key_name).unwrap_or_else(|| "—".into());
                            ui.monospace(name);
                        }
                        if ui.button("Rebind").clicked() {
                            self.rebinding = Some(action);
                        }
                        if ui.button("Clear").clicked() {
                            self.engine.bindings.set(action, None);
                            self.save_config();
                        }
                    });
                }
            });

            if !self.status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.status).color(theme::MUTED).small());
            }
        });
    }
}

fn meter(ui: &mut egui::Ui, label: &str, frac: f32, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized([90.0, 16.0], egui::Label::new(RichText::new(label).color(theme::MUTED).small()));
        let (rect, _) = ui.allocate_exact_size(Vec2::new(220.0, 14.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(3), theme::BG);
        let f = frac.clamp(0.0, 1.0);
        if f > 0.0 {
            let fill = Rect::from_min_size(rect.min, Vec2::new(rect.width() * f, rect.height()));
            painter.rect_filled(fill, CornerRadius::same(3), color);
        }
        ui.label(RichText::new(format!("{:.0}%", f * 100.0)).small().color(theme::MUTED));
    });
}

/// Insert or update a persisted (param id, value) in a VST node's list.
fn upsert_param(stored: &mut Vec<(u32, f64)>, id: u32, value: f64) {
    if let Some(entry) = stored.iter_mut().find(|(pid, _)| *pid == id) {
        entry.1 = value;
    } else {
        stored.push((id, value));
    }
}

/// Display label for a node — the plugin name for VST nodes, else the kind.
fn node_label(params: &NodeParams) -> String {
    match params {
        NodeParams::Vst3 { name, .. } => name.clone(),
        other => other.kind().label().to_string(),
    }
}

/// Draw a connection wire as a gentle horizontal S-curve (two segments).
fn wire(painter: &egui::Painter, from: Pos2, to: Pos2) {
    let mid = Pos2::new((from.x + to.x) * 0.5, (from.y + to.y) * 0.5);
    let stroke = Stroke::new(2.0, theme::CYAN.gamma_multiply(0.8));
    painter.line_segment([from, mid], stroke);
    painter.line_segment([mid, to], stroke);
}
