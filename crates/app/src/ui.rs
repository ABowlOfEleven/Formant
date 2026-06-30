//! The themed eframe UI: tabbed Mixer / Nodes / Setup, with hover tooltips.
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

const NODE_SIZE: Vec2 = Vec2::new(170.0, 66.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mixer,
    Nodes,
    Setup,
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
    zoom: f32,
    // VST3 state.
    plugins: Vec<formant_vst3::Plugin>,
    installed_vsts: HashSet<NodeId>,
    node_params: HashMap<NodeId, Vec<VstParam>>,
    editors: HashMap<NodeId, formant_vst3::PluginEditor>,
    // Tray + session.
    tray: Option<crate::tray::Tray>,
    quitting: bool,
    session_dirty: bool,
    last_session_save: std::time::Instant,
    // Tracks tab transitions so we can re-scan devices on entering Setup.
    last_tab: Option<Tab>,
    // Throttles audio device-loss recovery attempts.
    last_recovery: Option<std::time::Instant>,
}

/// A VST node parameter as shown in the inspector (normalized value).
struct VstParam {
    id: u32,
    name: String,
    value: f32,
}

impl FormantApp {
    pub fn new(config: Config, engine: Engine, com: formant_audio::com::ComGuard, graph: Graph) -> Self {
        let mut app = Self {
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
            zoom: 1.0,
            plugins: Vec::new(),
            installed_vsts: HashSet::new(),
            node_params: HashMap::new(),
            editors: HashMap::new(),
            tray: crate::tray::build(),
            quitting: false,
            session_dirty: false,
            last_session_save: std::time::Instant::now(),
            last_tab: None,
            last_recovery: None,
        };
        // Scan devices up front so the virtual-cable check is ready on first frame.
        app.refresh_devices();
        app
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

    /// Show whether a virtual audio cable is installed, and help the user get one
    /// or select it. This is the stopgap until Formant ships its own virtual audio
    /// driver: without a cable, other apps cannot read Formant as a microphone.
    fn virtual_cable_notice(&mut self, ui: &mut egui::Ui) {
        use formant_audio::devices::{detect_cable, KNOWN_CABLES};

        if self.render_devices.is_empty() {
            let mut recheck = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Virtual cable: not checked yet.").color(theme::MUTED).small());
                recheck |= ui.button("Check now").clicked();
            });
            if recheck {
                self.refresh_devices();
            }
            return;
        }

        // `detect_cable` returns a borrow; copy it out so we can mutate self below.
        let detected = detect_cable(&self.render_devices).copied();
        let mut open_url: Option<&'static str> = None;
        let mut recheck = false;
        let mut add_output: Option<String> = None;

        match detected {
            Some(cable) => {
                let full = self
                    .render_devices
                    .iter()
                    .find(|d| d.to_lowercase().contains(&cable.render_hint.to_lowercase()))
                    .cloned();
                let already = match &full {
                    Some(name) => self.config.devices.outputs.iter().any(|o| {
                        !o.is_empty() && name.to_lowercase().contains(&o.to_lowercase())
                    }),
                    None => true,
                };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("● Virtual cable found: {}", cable.name))
                            .color(theme::GOOD)
                            .small(),
                    );
                    if !already {
                        if let Some(name) = &full {
                            if ui
                                .button("Use as output")
                                .on_hover_text("Add this cable as a Formant output and restart, so other apps hear you.")
                                .clicked()
                            {
                                add_output = Some(name.clone());
                            }
                        }
                    }
                });
            }
            None => {
                ui.label(RichText::new("No virtual audio cable detected").color(theme::EMBER).strong());
                ui.label(
                    RichText::new("Other apps cannot read Formant as a microphone until you install a virtual cable. VB-CABLE is free and installs in a minute.")
                        .color(theme::MUTED)
                        .small(),
                );
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Get VB-CABLE").color(theme::CYAN)).clicked() {
                        open_url = Some(KNOWN_CABLES[0].url);
                    }
                    recheck |= ui.button("Re-check").on_hover_text("Scan the audio devices again after installing.").clicked();
                });
            }
        }
        ui.add_space(2.0);

        if let Some(url) = open_url {
            crate::platform::open_url(url);
        }
        if recheck {
            self.refresh_devices();
        }
        if let Some(name) = add_output {
            self.config.devices.outputs.push(name);
            self.restart_engine();
        }
    }

    /// First-run welcome overlay: explain the routing model and point at Setup.
    fn welcome_overlay(&mut self, ctx: &egui::Context) {
        if self.config.seen_welcome {
            return;
        }
        let mut dismiss = false;
        let mut go_setup = false;
        egui::Window::new("Welcome to Formant")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label("Formant cleans up and shapes your microphone in real time, then sends it to two places at once:");
                ui.add_space(4.0);
                ui.label(RichText::new("- your headphones, so you can hear yourself").small());
                ui.label(RichText::new("- a virtual cable that other apps read as a microphone").small());
                ui.add_space(8.0);
                ui.label("To use it in Discord, OBS, or a game:");
                ui.label(RichText::new("1. In Setup, pick your mic, your headphones, and your virtual cable.").small());
                ui.label(RichText::new("2. In the other app, set the microphone to that same cable.").small());
                ui.add_space(8.0);
                if formant_audio::devices::detect_cable(&self.render_devices).is_none() {
                    ui.label(
                        RichText::new("No virtual cable is installed yet. Setup has a one-click link to get one (VB-CABLE is free).")
                            .color(theme::EMBER)
                            .small(),
                    );
                    ui.add_space(8.0);
                }
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Open Setup").color(theme::CYAN)).clicked() {
                        go_setup = true;
                        dismiss = true;
                    }
                    if ui.button("Got it").clicked() {
                        dismiss = true;
                    }
                });
            });
        if go_setup {
            self.tab = Tab::Setup;
        }
        if dismiss {
            self.config.seen_welcome = true;
            let _ = self.config.save();
        }
    }

    /// Export a preset to a user-chosen `.ron` file (to share/back up).
    fn export_preset(&mut self, preset: &Preset) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("{}.ron", preset.name))
            .add_filter("Formant preset", &["ron"])
            .save_file()
        else {
            return;
        };
        self.status = match preset.to_ron().and_then(|s| Ok(std::fs::write(&path, s)?)) {
            Ok(_) => format!("exported '{}'", preset.name),
            Err(e) => format!("export failed: {e}"),
        };
    }

    /// Import a `.ron` preset from anywhere into the presets folder.
    fn import_preset(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("Formant preset", &["ron"]).pick_file() else {
            return;
        };
        match std::fs::read_to_string(&path).ok().and_then(|s| Preset::from_ron(&s).ok()) {
            Some(p) => {
                let _ = p.save();
                self.presets = Preset::load_all();
                self.status = format!("imported '{}'", p.name);
            }
            None => self.status = "import failed (not a valid preset)".into(),
        }
    }
}

impl eframe::App for FormantApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint(); // live meters
        let ctx = ui.ctx().clone();

        // Tray actions.
        for action in crate::tray::poll() {
            match action {
                crate::tray::TrayAction::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                crate::tray::TrayAction::ToggleBypass => {
                    self.engine.controls.toggle_bypass();
                }
                crate::tray::TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // Recover from a lost audio device (unplug, disable, default change) by
        // restarting the engine, throttled so a truly-gone device doesn't loop.
        if self.engine.device_lost() {
            let due = self.last_recovery.map_or(true, |t| t.elapsed().as_secs_f32() > 3.0);
            if due {
                crate::logging::line("audio device lost; restarting the engine to recover");
                self.restart_engine();
                self.last_recovery = Some(std::time::Instant::now());
                self.refresh_devices();
            }
        }

        // Closing the window hides to the tray instead of quitting.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            if self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                // Reassure the user the first time, so it doesn't seem stuck.
                if !self.config.seen_tray_hint {
                    crate::platform::notify(
                        "Formant",
                        "Formant is still running in the system tray, so your processed mic keeps working. Click the tray icon to bring the window back, or use Quit there to exit.",
                    );
                    self.config.seen_tray_hint = true;
                    let _ = self.config.save();
                }
            } else {
                self.quitting = true;
            }
        }

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
        // Re-scan devices each time the user enters Setup, so the virtual-cable
        // check reflects anything they just installed.
        if self.tab == Tab::Setup && self.last_tab != Some(Tab::Setup) {
            self.refresh_devices();
        }
        self.last_tab = Some(self.tab);
        egui::CentralPanel::default().show_inside(ui, |ui| {
            paint_backdrop(ui);
            match self.tab {
                Tab::Mixer => self.tab_mixer(ui),
                Tab::Nodes => self.tab_nodes(ui),
                Tab::Setup => self.tab_setup(ui),
            }
        });
        self.welcome_overlay(&ctx);

        if self.graph != self.last_pushed {
            self.engine.push_graph(&self.graph);
            self.last_pushed = self.graph.clone();
            self.session_dirty = true;
        }
        self.reconcile_vsts();
        self.drain_gui_edits();

        // Persist the working graph as the session, throttled.
        if self.session_dirty && self.last_session_save.elapsed().as_secs_f32() > 1.5 {
            let _ = formant_core::session::save(&self.graph);
            self.session_dirty = false;
            self.last_session_save = std::time::Instant::now();
        }
    }
}

impl Drop for FormantApp {
    fn drop(&mut self) {
        // Save the final session graph on exit.
        let _ = formant_core::session::save(&self.graph);
    }
}

impl FormantApp {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let needs_cable = !self.render_devices.is_empty()
            && formant_audio::devices::detect_cable(&self.render_devices).is_none();
        let audio_lost = self.engine.device_lost();
        egui::Panel::top("top").show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("FORMANT").color(theme::CYAN).strong().size(22.0));
                ui.label(RichText::new("· vocal processor").color(theme::MUTED).small());
                ui.add_space(14.0);
                for (tab, name) in [
                    (Tab::Mixer, "Mixer"),
                    (Tab::Nodes, "Nodes"),
                    (Tab::Setup, "Setup"),
                ] {
                    if tab_button(ui, name, self.tab == tab) {
                        self.tab = tab;
                    }
                }
                if audio_lost {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("● audio device changed, reconnecting")
                            .color(theme::EMBER)
                            .small(),
                    )
                    .on_hover_text("An audio device was unplugged or changed. Formant is restarting the stream.");
                } else if needs_cable {
                    ui.add_space(10.0);
                    if ui
                        .button(RichText::new("● no virtual cable").color(theme::EMBER).small())
                        .on_hover_text("Apps cannot read Formant as a microphone until a virtual cable is installed. Click to open Setup.")
                        .clicked()
                    {
                        self.tab = Tab::Setup;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let bypassed = self.engine.controls.global_bypass();
                    let (txt, col) = if bypassed {
                        ("● BYPASSED", theme::EMBER)
                    } else {
                        ("● live", theme::GOOD)
                    };
                    if ui.button(RichText::new(txt).color(col).strong()).on_hover_text("Turn ALL processing off - passes your raw mic straight through.").clicked() {
                        self.engine.controls.toggle_bypass();
                    }
                });
            });
            ui.add_space(6.0);
            // Accent rule under the header.
            let r = ui.max_rect();
            let y = r.bottom();
            ui.painter().line_segment(
                [egui::pos2(r.left(), y), egui::pos2(r.right(), y)],
                egui::Stroke::new(1.5, theme::CYAN.gamma_multiply(0.5)),
            );
        });
    }

    fn tab_mixer(&mut self, ui: &mut egui::Ui) {
        let (in_peak, out_peak, vad, gr) = {
            let m = &self.engine.meters;
            (m.in_peak(), m.out_peak(), m.vad(), m.gain_reduction_db())
        };

        // -- Meters + transport -------------------------------------------
        theme::card(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("MIXER").color(theme::CYAN));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let bypassed = self.engine.controls.global_bypass();
                    let (txt, col) = if bypassed { ("● BYPASSED", theme::EMBER) } else { ("● live", theme::GOOD) };
                    if ui.button(RichText::new(txt).color(col).strong()).on_hover_text("Turn ALL processing off - passes your raw mic straight through.").clicked() {
                        self.engine.controls.toggle_bypass();
                    }
                });
            });
            ui.add_space(4.0);
            meter(ui, "input", in_peak, "Incoming microphone level.");
            meter(ui, "output", out_peak, "Processed level sent to your monitor and the virtual mic.");
            meter(ui, "voice (VAD)", vad, "How confident the AI is that you're speaking right now.");
            meter(ui, "comp GR", (gr / 20.0).clamp(0.0, 1.0), "How much the compressor is currently turning the level down.");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("mute").color(theme::MUTED).small())
                    .on_hover_text("How the mic decides when to open/close.");
                let cur = self.engine.controls.mode();
                for m in [MuteMode::Vad, MuteMode::PushToTalk, MuteMode::Toggle, MuteMode::AlwaysOpen] {
                    if ui.selectable_label(cur == m, m.label()).on_hover_text(mode_help(m)).clicked() {
                        self.engine.controls.set_mode(m);
                    }
                }
            });
        });

        // -- Preset quick-switch ------------------------------------------
        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("PRESETS").color(theme::CYAN).strong());
                ui.add(egui::TextEdit::singleline(&mut self.new_preset_name).hint_text("name").desired_width(120.0));
                if ui.button("Save").clicked() && !self.new_preset_name.trim().is_empty() {
                    let preset = Preset::new(self.new_preset_name.trim(), self.graph.clone());
                    self.status = match preset.save() {
                        Ok(_) => format!("saved '{}'", preset.name),
                        Err(e) => format!("save failed: {e}"),
                    };
                    self.new_preset_name.clear();
                    self.presets = Preset::load_all();
                }
            });
            if self.presets.is_empty() {
                ui.label(RichText::new("no presets - save one, or import in Setup").color(theme::MUTED).small());
            }
            let presets = self.presets.clone();
            ui.horizontal_wrapped(|ui| {
                for preset in presets {
                    if ui.button(&preset.name).clicked() {
                        self.graph = preset.graph.clone();
                        self.selected = None;
                        self.status = format!("loaded '{}'", preset.name);
                    }
                }
            });
        });

        // -- Channel strip (one channel per chain node, in signal order) --
        ui.add_space(8.0);
        theme::card(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("CHANNEL STRIP").color(theme::CYAN).strong());
            let ids: Vec<NodeId> = self
                .graph
                .exec_order()
                .into_iter()
                .filter(|id| !matches!(self.graph.kind_of(*id), Some(NodeKind::Input) | Some(NodeKind::Output)))
                .collect();
            if ids.is_empty() {
                ui.label(RichText::new("empty chain - add nodes in the Nodes tab").color(theme::MUTED).small());
            }
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for id in ids {
                        self.channel(ui, id);
                    }
                });
            });
        });
    }

    /// One mixer channel for a chain node: name (→ select in Nodes), enable,
    /// and its primary parameter.
    fn channel(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(node) = self.graph.node(id).cloned() else { return };
        let accent = node_accent(node.params.kind());
        theme::card(ui.style()).show(ui, |ui| {
            ui.set_width(132.0);
            ui.push_id(id, |ui| {
                ui.vertical(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new(node_label(&node.params)).color(accent).strong()).frame(false))
                        .on_hover_text(format!("{}\n\nClick to edit this node in the Nodes tab.", kind_help(node.params.kind())))
                        .clicked()
                    {
                        self.selected = Some(id);
                        self.tab = Tab::Nodes;
                    }
                    let mut on = !node.bypass;
                    if ui.checkbox(&mut on, "on").changed() {
                        if let Some(n) = self.graph.node_mut(id) {
                            n.bypass = !on;
                        }
                    }
                    let mut params = node.params.clone();
                    if primary_slider(ui, &mut params) {
                        if let Some(n) = self.graph.node_mut(id) {
                            n.params = params;
                        }
                    }
                });
            });
        });
    }

    fn tab_nodes(&mut self, ui: &mut egui::Ui) {
        // Inspector lives in a resizable right panel (drag its left edge); its
        // contents scroll so nothing clips.
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(280.0)
            .size_range(220.0..=520.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.draw_inspector(ui));
            });
        // The graph canvas fills whatever space is left.
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let size = ui.available_size();
            let (canvas, bg) = ui.allocate_exact_size(size, Sense::click_and_drag());
            self.draw_canvas(ui, canvas, bg);
        });
    }

    fn draw_canvas(&mut self, ui: &mut egui::Ui, canvas: Rect, bg: egui::Response) {
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, CornerRadius::same(8), theme::BG);

        if self.dragging_from.is_none() && bg.dragged() {
            self.pan += bg.drag_delta();
        }
        if bg.clicked() {
            self.selected = None;
        }

        // Scroll-wheel zoom, anchored at the cursor.
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            if let Some(cur) = pointer {
                if canvas.contains(cur) {
                    let old = self.zoom;
                    let new = (old * (scroll * 0.0015).exp()).clamp(0.35, 3.0);
                    let world = (cur - canvas.min - self.pan) / old;
                    self.pan = (cur - canvas.min) - world * new;
                    self.zoom = new;
                }
            }
        }
        let zoom = self.zoom;
        let ns = NODE_SIZE * zoom;
        let origin = canvas.min.to_vec2() + self.pan;
        let to_screen = |p: [f32; 2]| Pos2::new(origin.x + p[0] * zoom, origin.y + p[1] * zoom);

        // Glowing dot grid (pans + scales with the canvas).
        let step = (28.0 * zoom).max(16.0);
        let glow = theme::CYAN.gamma_multiply(0.07);
        let dot = theme::CYAN.gamma_multiply(0.22);
        let ox = self.pan.x.rem_euclid(step);
        let oy = self.pan.y.rem_euclid(step);
        let mut gx = canvas.min.x + ox;
        while gx < canvas.max.x {
            let mut gy = canvas.min.y + oy;
            while gy < canvas.max.y {
                let c = Pos2::new(gx, gy);
                painter.circle_filled(c, 2.6, glow);
                painter.circle_filled(c, 1.1, dot);
                gy += step;
            }
            gx += step;
        }

        // Snapshot positions/kinds so we don't borrow the graph while mutating it.
        let layout: HashMap<NodeId, (Pos2, NodeKind, bool)> = self
            .graph
            .nodes
            .iter()
            .map(|n| (n.id, (to_screen(n.pos), n.params.kind(), n.bypass)))
            .collect();
        let ids: Vec<NodeId> = self.graph.nodes.iter().map(|n| n.id).collect();
        let labels: HashMap<NodeId, String> = self
            .graph
            .nodes
            .iter()
            .map(|n| (n.id, node_label(&n.params)))
            .collect();

        // Wires (under nodes).
        for c in &self.graph.connections.clone() {
            if let (Some(&(fp, ..)), Some(&(tp, ..))) = (layout.get(&c.from), layout.get(&c.to)) {
                wire(&painter, fp + Vec2::new(ns.x, ns.y * 0.5), tp + Vec2::new(0.0, ns.y * 0.5), theme::CYAN.gamma_multiply(0.85));
            }
        }
        if let Some(fid) = self.dragging_from {
            if let (Some(&(fp, ..)), Some(ptr)) = (layout.get(&fid), ui.input(|i| i.pointer.interact_pos())) {
                wire(&painter, fp + Vec2::new(ns.x, ns.y * 0.5), ptr, theme::EMBER);
            }
        }

        // Nodes.
        for id in &ids {
            let id = *id;
            let &(pos, kind, bypass) = layout.get(&id).unwrap();
            let rect = Rect::from_min_size(pos, ns);

            let resp = ui
                .interact(rect, egui::Id::new(("fmt_node", id)), Sense::click_and_drag())
                .on_hover_text(kind_help(kind));
            if resp.dragged() {
                if let Some(n) = self.graph.node_mut(id) {
                    let d = resp.drag_delta() / zoom; // screen -> world
                    n.pos[0] += d.x;
                    n.pos[1] += d.y;
                }
            }
            if resp.clicked() {
                self.selected = Some(id);
            }

            let selected = self.selected == Some(id);
            let accent = node_accent(kind);
            let r6 = CornerRadius::same((8.0 * zoom) as u8);

            painter.rect_filled(rect.translate(Vec2::new(0.0, 3.0 * zoom)), r6, Color32::from_black_alpha(80));
            painter.rect_filled(rect, r6, if selected { theme::CARD_HI } else { theme::CARD });
            let strip = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 6.0 * zoom));
            let sr = (8.0 * zoom) as u8;
            painter.rect_filled(strip, CornerRadius { nw: sr, ne: sr, sw: 0, se: 0 }, accent.gamma_multiply(if bypass { 0.3 } else { 0.95 }));
            let border = if selected {
                accent
            } else if resp.hovered() {
                theme::lerp(theme::LINE, accent, 0.6)
            } else {
                theme::LINE
            };
            painter.rect_stroke(rect, r6, Stroke::new(if selected { 2.0 } else { 1.0 }, border), StrokeKind::Inside);

            let title_col = if bypass { theme::MUTED } else { theme::TEXT };
            let label = labels.get(&id).cloned().unwrap_or_else(|| kind.label().to_string());
            painter.text(rect.min + Vec2::new(13.0 * zoom, 16.0 * zoom), Align2::LEFT_TOP, label, FontId::proportional(15.0 * zoom), title_col);
            let sub = if bypass {
                Some(("bypassed", theme::EMBER))
            } else if kind == NodeKind::Vst3 {
                Some(("vst3", theme::EMBER))
            } else {
                None
            };
            if let Some((s, c)) = sub {
                painter.text(rect.min + Vec2::new(13.0 * zoom, 40.0 * zoom), Align2::LEFT_TOP, s, FontId::proportional(10.0 * zoom), c);
            }

            let pr = 7.0 * zoom;
            let pin = 5.0 * zoom;
            let hit = Vec2::splat(18.0 * zoom);
            if kind != NodeKind::Input {
                let p = rect.left_center();
                painter.circle_filled(p, pr, theme::BG);
                painter.circle_filled(p, pin, theme::EMBER);
                if ui.interact(Rect::from_center_size(p, hit), egui::Id::new(("fmt_in", id)), Sense::click()).clicked() {
                    self.graph.disconnect_into(id);
                }
            }
            if kind != NodeKind::Output {
                let p = rect.right_center();
                painter.circle_filled(p, pr, theme::BG);
                painter.circle_filled(p, pin, theme::CYAN);
                if ui.interact(Rect::from_center_size(p, hit), egui::Id::new(("fmt_out", id)), Sense::drag()).drag_started() {
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
                        Rect::from_center_size(Rect::from_min_size(*pos, ns).left_center(), Vec2::splat(20.0 * zoom)).contains(ptr)
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
                    if ui.button(kind.label()).on_hover_text(kind_help(kind)).clicked() {
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
                self.zoom = 1.0;
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
                    ui.add(egui::Slider::new(cutoff_hz, 20.0..=400.0).text("cutoff Hz"))
                        .on_hover_text("Frequencies below this are removed. ~80-120 Hz clears rumble without thinning your voice.");
                }
                NodeParams::Gate { threshold_db, range_db, attack_ms, hold_ms, release_ms, vad_gate } => {
                    ui.add(egui::Slider::new(threshold_db, -80.0..=0.0).text("threshold dB"))
                        .on_hover_text("How loud you must be for the gate to open. Set it just above your background noise.");
                    ui.add(egui::Slider::new(range_db, -90.0..=0.0).text("range dB"))
                        .on_hover_text("How much the mic is quieted when closed. More negative = closer to fully silent.");
                    ui.add(egui::Slider::new(attack_ms, 0.1..=50.0).text("attack ms"))
                        .on_hover_text("How quickly the gate opens when you start talking.");
                    ui.add(egui::Slider::new(hold_ms, 0.0..=500.0).text("hold ms"))
                        .on_hover_text("How long it stays open after you stop, so word endings aren't cut off.");
                    ui.add(egui::Slider::new(release_ms, 5.0..=1000.0).logarithmic(true).text("release ms"))
                        .on_hover_text("How slowly the gate closes after the hold - longer is more natural.");
                    ui.checkbox(vad_gate, "follow VAD (RNNoise)")
                        .on_hover_text("Open the gate from AI voice detection instead of raw loudness.");
                }
                NodeParams::DeEsser { threshold_db, ratio, split_hz } => {
                    ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("threshold dB"))
                        .on_hover_text("How loud sibilance must be before it's tamed. Lower = more de-essing.");
                    ui.add(egui::Slider::new(ratio, 1.0..=12.0).text("ratio"))
                        .on_hover_text("How hard the 'ess' is reduced once over threshold.");
                    ui.add(egui::Slider::new(split_hz, 3000.0..=12000.0).text("split Hz"))
                        .on_hover_text("The frequency where 'ess'/'sh' sounds live (try 5-8 kHz). Only this band is touched.");
                }
                NodeParams::Compressor { threshold_db, ratio } => {
                    ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("threshold dB"))
                        .on_hover_text("Level above which compression kicks in. Lower = more of your voice gets evened out.");
                    ui.add(egui::Slider::new(ratio, 1.0..=20.0).text("ratio"))
                        .on_hover_text("How hard loud parts are turned down. 3:1 means 3 dB over → 1 dB out.");
                }
                NodeParams::Eq { low_db, mid_db, high_db } => {
                    ui.add(egui::Slider::new(low_db, -12.0..=12.0).text("low dB"))
                        .on_hover_text("Boost (+) or cut (-) the low end / warmth (~120 Hz).");
                    ui.add(egui::Slider::new(mid_db, -12.0..=12.0).text("mid dB"))
                        .on_hover_text("Boost (+) or cut (-) the mids / presence (~3 kHz).");
                    ui.add(egui::Slider::new(high_db, -12.0..=12.0).text("high dB"))
                        .on_hover_text("Boost (+) or cut (-) the highs / air (~10 kHz).");
                }
                NodeParams::Saturator { drive, mix } => {
                    ui.add(egui::Slider::new(drive, 1.0..=8.0).text("drive"))
                        .on_hover_text("How hard the signal is pushed - more = warmer, then grittier.");
                    ui.add(egui::Slider::new(mix, 0.0..=1.0).text("mix"))
                        .on_hover_text("Blend between clean (0) and saturated (1).");
                }
                NodeParams::Limiter { ceiling_db } => {
                    ui.add(egui::Slider::new(ceiling_db, -24.0..=0.0).text("ceiling dB"))
                        .on_hover_text("Maximum output level - nothing gets louder than this.");
                }
                NodeParams::Gain { gain_db } | NodeParams::Makeup { gain_db } => {
                    ui.add(egui::Slider::new(gain_db, -24.0..=24.0).text("gain dB"))
                        .on_hover_text("Volume adjustment in decibels (+ louder, - quieter).");
                }
                NodeParams::Mix { gain_db } => {
                    ui.add(egui::Slider::new(gain_db, -24.0..=24.0).text("output dB"))
                        .on_hover_text("Output trim after summing every wire feeding this node.");
                }
                NodeParams::Vst3 { name, params: stored, .. } => {
                    ui.label(RichText::new(name.as_str()).color(theme::MUTED));
                    if self.editors.contains_key(&id)
                        && ui.button(RichText::new("Open plugin editor").color(theme::CYAN)).on_hover_text("Open the plugin's own window with all its native controls.").clicked()
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
                                    ui.horizontal(|ui| {
                                        let resp = ui.add(
                                            egui::Slider::new(&mut p.value, 0.0..=1.0)
                                                .show_value(false)
                                                .text(&p.name),
                                        );
                                        if resp.changed() {
                                            let v = p.value as f64;
                                            self.engine.set_effect_param(id, p.id, v);
                                            if let Some(ed) = self.editors.get(&id) {
                                                ed.set_param(p.id, v);
                                            }
                                            upsert_param(stored, p.id, v);
                                        }
                                        // The plugin's own formatted value (e.g. "-3 dB").
                                        let plain = self
                                            .editors
                                            .get(&id)
                                            .map(|ed| ed.param_string(p.id, p.value as f64))
                                            .unwrap_or_default();
                                        if !plain.is_empty() {
                                            ui.label(RichText::new(plain).color(theme::EMBER).small());
                                        }
                                    });
                                }
                            });
                        }
                        Some(_) => {
                            ui.label(RichText::new("no editable parameters").color(theme::MUTED).small());
                        }
                        None => {
                            ui.label(RichText::new("loading...").color(theme::MUTED).small());
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

    fn tab_setup(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            // -- Presets --------------------------------------------------
            theme::card(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("PRESETS").color(theme::CYAN).strong());
                    if ui.button("Refresh").clicked() {
                        self.presets = Preset::load_all();
                    }
                    if ui.button("Import...").on_hover_text("Load a .ron preset file from anywhere into your library.").clicked() {
                        self.import_preset();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.new_preset_name).hint_text("save current as..."));
                    if ui.button("Save").clicked() && !self.new_preset_name.trim().is_empty() {
                        let preset = Preset::new(self.new_preset_name.trim(), self.graph.clone());
                        self.status = match preset.save() {
                            Ok(_) => format!("saved '{}'", preset.name),
                            Err(e) => format!("save failed: {e}"),
                        };
                        self.new_preset_name.clear();
                        self.presets = Preset::load_all();
                    }
                });
                if self.presets.is_empty() {
                    ui.label(RichText::new("no presets yet").color(theme::MUTED).small());
                }
                let mut to_delete: Option<usize> = None;
                let mut to_export: Option<usize> = None;
                for (i, preset) in self.presets.clone().into_iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(&preset.name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Delete").clicked() {
                                let _ = preset.delete();
                                to_delete = Some(i);
                            }
                            if ui.button("Export...").on_hover_text("Save this preset to a .ron file to share or back up.").clicked() {
                                to_export = Some(i);
                            }
                            if ui.button("Load").clicked() {
                                self.graph = preset.graph.clone();
                                self.selected = None;
                                self.status = format!("loaded '{}'", preset.name);
                            }
                        });
                    });
                }
                if let Some(i) = to_export {
                    let p = self.presets[i].clone();
                    self.export_preset(&p);
                }
                if let Some(i) = to_delete {
                    self.presets.remove(i);
                }
            });

            ui.add_space(8.0);
            theme::card(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("DEVICES").color(theme::CYAN).strong());
                self.virtual_cable_notice(ui);
                ui.label(RichText::new(format!("mic now: {}", self.engine.mic_name)).color(theme::MUTED).small());

                // Clone the device lists so the dropdown closures don't fight the
                // borrow checker over self.
                let caps = self.capture_devices.clone();
                let rends = self.render_devices.clone();

                ui.horizontal(|ui| {
                    ui.label("mic");
                    let cur = self.config.devices.mic.clone();
                    egui::ComboBox::from_id_salt("mic-select")
                        .selected_text(if cur.is_empty() { "(choose a microphone)".into() } else { cur })
                        .width(360.0)
                        .show_ui(ui, |ui| {
                            for d in &caps {
                                ui.selectable_value(&mut self.config.devices.mic, d.clone(), d);
                            }
                        });
                });

                let mut remove: Option<usize> = None;
                for i in 0..self.config.devices.outputs.len() {
                    ui.horizontal(|ui| {
                        ui.label("out");
                        let cur = self.config.devices.outputs[i].clone();
                        egui::ComboBox::from_id_salt(("out-select", i))
                            .selected_text(if cur.is_empty() { "(choose an output)".into() } else { cur })
                            .width(360.0)
                            .show_ui(ui, |ui| {
                                for d in &rends {
                                    ui.selectable_value(&mut self.config.devices.outputs[i], d.clone(), d);
                                }
                            });
                        if ui.button("✕").on_hover_text("Remove this output").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.config.devices.outputs.remove(i);
                }

                ui.horizontal(|ui| {
                    if ui.button("+ output").on_hover_text("Add another device to send the processed signal to.").clicked() {
                        self.config.devices.outputs.push(String::new());
                    }
                    if ui.button("Apply & restart").on_hover_text("Switch to the selected devices.").clicked() {
                        self.restart_engine();
                    }
                    if ui.button("Rescan").on_hover_text("Re-list the audio devices.").clicked() {
                        self.refresh_devices();
                    }
                });

                ui.collapsing("Advanced (match by name)", |ui| {
                    ui.label(
                        RichText::new("Formant matches devices by case-insensitive substring, so a partial name like \"CABLE Input\" keeps working even if the full name changes.")
                            .color(theme::MUTED)
                            .small(),
                    );
                    ui.horizontal(|ui| {
                        ui.label("mic");
                        ui.add(egui::TextEdit::singleline(&mut self.config.devices.mic));
                    });
                    for i in 0..self.config.devices.outputs.len() {
                        ui.horizontal(|ui| {
                            ui.label("out");
                            ui.add(egui::TextEdit::singleline(&mut self.config.devices.outputs[i]));
                        });
                    }
                });
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
                            ui.colored_label(theme::EMBER, "press a key...  (Esc cancels)");
                        } else {
                            let name = self.engine.bindings.get(action).map(hotkeys::key_name).unwrap_or_else(|| "-".into());
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

            ui.add_space(8.0);
            theme::card(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("STARTUP").color(theme::CYAN).strong());
                let mut autostart = crate::platform::autostart_enabled();
                if ui
                    .checkbox(&mut autostart, "Start Formant when Windows starts")
                    .on_hover_text("Launch Formant automatically at login, so your processed mic is always ready.")
                    .changed()
                {
                    self.status = match crate::platform::set_autostart(autostart) {
                        Ok(_) if autostart => "autostart enabled".into(),
                        Ok(_) => "autostart disabled".into(),
                        Err(e) => format!("autostart failed: {e}"),
                    };
                }
                ui.label(
                    RichText::new("Closing the window hides Formant to the system tray; use the tray icon (or Quit) to bring it back.")
                        .color(theme::MUTED)
                        .small(),
                );
            });

            ui.add_space(8.0);
            theme::card(ui.style()).show(ui, |ui| {
                ui.label(RichText::new("ABOUT").color(theme::CYAN).strong());
                ui.label(
                    RichText::new(format!("Formant {}", env!("CARGO_PKG_VERSION")))
                        .color(theme::MUTED)
                        .small(),
                );
                ui.horizontal(|ui| {
                    if ui.button("Check for updates").on_hover_text("Opens the Releases page in your browser. Formant never checks on its own.").clicked() {
                        crate::platform::open_url("https://github.com/ABowlOfEleven/Formant/releases");
                    }
                    if ui.button("View on GitHub").clicked() {
                        crate::platform::open_url("https://github.com/ABowlOfEleven/Formant");
                    }
                });
            });

            if !self.status.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&self.status).color(theme::MUTED).small());
            }
        });
    }
}

/// A segmented LED-style level meter (cyan at low, ember toward the top).
fn meter(ui: &mut egui::Ui, label: &str, frac: f32, help: &str) {
    let frac = frac.clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.add_sized([92.0, 16.0], egui::Label::new(RichText::new(label).color(theme::MUTED).small()))
            .on_hover_text(help);
        let segs = 26usize;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(228.0, 15.0), Sense::hover());
        resp.on_hover_text(help);
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(4), theme::BG);
        let gap = 2.0;
        let inset = 3.0;
        let seg_w = (rect.width() - 2.0 * inset - gap * (segs as f32 - 1.0)) / segs as f32;
        let lit = (frac * segs as f32).ceil() as usize;
        for i in 0..segs {
            let x = rect.min.x + inset + i as f32 * (seg_w + gap);
            let sr = Rect::from_min_size(Pos2::new(x, rect.min.y + inset), Vec2::new(seg_w, rect.height() - 2.0 * inset));
            let t = i as f32 / (segs as f32 - 1.0);
            let on = i < lit;
            let col = if on {
                theme::lerp(theme::CYAN, theme::EMBER, (t * 1.5 - 0.5).clamp(0.0, 1.0))
            } else {
                theme::lerp(theme::BG, theme::LINE, 0.5)
            };
            painter.rect_filled(sr, CornerRadius::same(1), col);
        }
        ui.label(RichText::new(format!("{:.0}%", frac * 100.0)).small().color(theme::MUTED));
    });
}

/// A pill-style tab button with a clear active state + accent underline.
fn tab_button(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(94.0, 30.0), Sense::click());
    let painter = ui.painter();
    let r = CornerRadius::same(7);
    if active {
        painter.rect_filled(rect, r, theme::lerp(theme::PANEL, theme::CYAN, 0.16));
    } else if resp.hovered() {
        painter.rect_filled(rect, r, theme::CARD);
    }
    let col = if active {
        theme::CYAN
    } else if resp.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    painter.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(14.5), col);
    if active {
        let bar = Rect::from_min_size(
            Pos2::new(rect.left() + 16.0, rect.bottom() - 3.0),
            Vec2::new(rect.width() - 32.0, 2.5),
        );
        painter.rect_filled(bar, CornerRadius::same(2), theme::CYAN);
    }
    resp.clicked()
}

/// A subtle top-light / bottom-dark gradient behind tab content, for depth.
fn paint_backdrop(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let n = 28;
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let y0 = rect.top() + rect.height() * i as f32 / n as f32;
        let y1 = rect.top() + rect.height() * (i + 1) as f32 / n as f32;
        let col = theme::lerp(theme::PANEL, theme::BG, t * 0.55);
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(rect.left(), y0), Pos2::new(rect.right(), y1)),
            0.0,
            col,
        );
    }
}

/// Render a node's primary parameter as a compact slider (mixer channel).
/// Returns true if it changed.
fn primary_slider(ui: &mut egui::Ui, params: &mut NodeParams) -> bool {
    let resp = match params {
        NodeParams::HighPass { cutoff_hz } => Some(ui.add(egui::Slider::new(cutoff_hz, 20.0..=400.0).text("Hz")).on_hover_text("Cutoff - frequencies below this are removed.")),
        NodeParams::Gate { threshold_db, .. } => Some(ui.add(egui::Slider::new(threshold_db, -80.0..=0.0).text("thr")).on_hover_text("Gate threshold - how loud you must be to open the mic.")),
        NodeParams::DeEsser { threshold_db, .. } => Some(ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("thr")).on_hover_text("De-ess threshold - lower tames more sibilance.")),
        NodeParams::Compressor { threshold_db, .. } => Some(ui.add(egui::Slider::new(threshold_db, -60.0..=0.0).text("thr")).on_hover_text("Compressor threshold - lower evens out more of your voice.")),
        NodeParams::Eq { mid_db, .. } => Some(ui.add(egui::Slider::new(mid_db, -12.0..=12.0).text("mid")).on_hover_text("Mid boost/cut (presence).")),
        NodeParams::Saturator { drive, .. } => Some(ui.add(egui::Slider::new(drive, 1.0..=8.0).text("drive")).on_hover_text("Saturation drive - more = warmer/grittier.")),
        NodeParams::Limiter { ceiling_db } => Some(ui.add(egui::Slider::new(ceiling_db, -24.0..=0.0).text("ceil")).on_hover_text("Output ceiling - nothing gets louder than this.")),
        NodeParams::Gain { gain_db } | NodeParams::Makeup { gain_db } | NodeParams::Mix { gain_db } => {
            Some(ui.add(egui::Slider::new(gain_db, -24.0..=24.0).text("dB")).on_hover_text("Volume in decibels."))
        }
        _ => {
            ui.label(RichText::new("-").color(theme::MUTED).small());
            None
        }
    };
    resp.map(|r| r.changed()).unwrap_or(false)
}

/// Plain-language explanation of what each node does (hover tooltip).
fn kind_help(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Input => "Your microphone - the start of the chain.",
        NodeKind::Output => "The processed signal sent to your monitor and the virtual mic apps use.",
        NodeKind::HighPass => "Removes low-frequency rumble (handling noise, thumps, AC hum) below the cutoff. Almost always worth having first.",
        NodeKind::Denoise => "AI noise reduction (RNNoise): removes steady background noise - fans, hiss, room tone - while keeping your voice.",
        NodeKind::Gate => "Silences the mic when you're not talking, so background noise doesn't leak through between words.",
        NodeKind::DeEsser => "Tames harsh 'sss' and 'shh' sounds (sibilance) without dulling the rest of your voice.",
        NodeKind::Compressor => "Evens out your volume - brings quiet parts up and loud parts down - for a steadier, fuller, more 'pro' sound.",
        NodeKind::Eq => "Tone control: boost or cut lows, mids, and highs to shape your voice.",
        NodeKind::Saturator => "Adds warmth and subtle harmonics like analog gear - gentle at low drive, gritty at high.",
        NodeKind::Limiter => "A safety ceiling: stops the output from ever getting too loud or clipping. Good as the last node.",
        NodeKind::Gain => "Simple volume trim - boost or cut the level at this point in the chain.",
        NodeKind::Makeup => "Final output volume - bring the processed signal back up to the level you want.",
        NodeKind::Mix => "Combines multiple wires into one by summing them. Wire several branches in for parallel chains: parallel compression, blend-in saturation, or dry/wet (control each branch's level with a Gain before it).",
        NodeKind::Vst3 => "A third-party VST3 plugin added to your chain. Open its editor for its own controls.",
    }
}

/// Plain-language explanation of each mute mode (hover tooltip).
fn mode_help(mode: MuteMode) -> &'static str {
    match mode {
        MuteMode::Vad => "Open the mic automatically when you talk, using AI voice detection. Hands-free.",
        MuteMode::PushToTalk => "Mic is open only while you hold the push-to-talk key (set it in Setup → Hotkeys).",
        MuteMode::Toggle => "Press the toggle key to mute/unmute (set it in Setup → Hotkeys).",
        MuteMode::AlwaysOpen => "Mic is always open - no gating at all.",
    }
}

/// Accent color for a node by kind.
fn node_accent(kind: NodeKind) -> Color32 {
    match kind {
        NodeKind::Input | NodeKind::Output => theme::MUTED,
        NodeKind::Vst3 => theme::EMBER,
        NodeKind::Mix => theme::GOOD,
        _ => theme::CYAN,
    }
}

/// Insert or update a persisted (param id, value) in a VST node's list.
fn upsert_param(stored: &mut Vec<(u32, f64)>, id: u32, value: f64) {
    if let Some(entry) = stored.iter_mut().find(|(pid, _)| *pid == id) {
        entry.1 = value;
    } else {
        stored.push((id, value));
    }
}

/// Display label for a node - the plugin name for VST nodes, else the kind.
fn node_label(params: &NodeParams) -> String {
    match params {
        NodeParams::Vst3 { name, .. } => name.clone(),
        other => other.kind().label().to_string(),
    }
}

/// Draw a connection wire as a smooth horizontal cubic bezier.
fn wire(painter: &egui::Painter, from: Pos2, to: Pos2, color: Color32) {
    let dx = ((to.x - from.x).abs() * 0.5).clamp(40.0, 160.0);
    let c1 = Pos2::new(from.x + dx, from.y);
    let c2 = Pos2::new(to.x - dx, to.y);
    let stroke = Stroke::new(2.5, color);
    let n = 24;
    let mut prev = from;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let pt = cubic(from, c1, c2, to, t);
        painter.line_segment([prev, pt], stroke);
        prev = pt;
    }
}

fn cubic(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    Pos2::new(
        a * p0.x + b * p1.x + c * p2.x + d * p3.x,
        a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    )
}
