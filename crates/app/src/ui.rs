//! The themed eframe UI: tabbed Mixer / Nodes / Setup, with hover tooltips.
//!
//! The Nodes tab is a hand-built drag-wire graph editor (canvas painted with the
//! `Painter`, interaction via `ui.interact`), with a side inspector for the
//! selected node's parameters.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};

use formant_audio::devices::{self, Direction};
use formant_audio::{hotkeys, Action};
use formant_core::{Config, Graph, MuteMode, NodeId, NodeKind, NodeParams, Preset};

use crate::engine::Engine;
use crate::hw;
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
    // Undo/redo of the graph. `undo_baseline` is the last committed snapshot;
    // edits are committed to history once they settle.
    undo_stack: Vec<Graph>,
    redo_stack: Vec<Graph>,
    undo_baseline: Graph,
    edit_pending_since: Option<std::time::Instant>,
    // Spectrum analyzer (Mixer tab).
    spectrum: crate::spectrum::Spectrum,
    scope_buf: Vec<f32>,
    // Gate auto-calibration in progress, if any.
    calibration: Option<Calibration>,
    // Cached autostart state (querying it spawns a process, so we don't do it
    // every frame; refreshed on entering Setup).
    autostart_on: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CalPhase {
    Quiet,
    Talk,
}

struct Calibration {
    phase: CalPhase,
    since: std::time::Instant,
    noise_sum: f32,
    noise_n: u32,
    voice_sum: f32,
    voice_n: u32,
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
            last_pushed: graph.clone(),
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
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            undo_baseline: graph,
            edit_pending_since: None,
            spectrum: crate::spectrum::Spectrum::new(),
            scope_buf: Vec::new(),
            calibration: None,
            autostart_on: false,
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
        /// A Vst3 node to keep loaded: its id, binary path, and stored params.
        type VstWant = (NodeId, String, Vec<(u32, f64)>);
        let want: Vec<VstWant> = self
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

    /// Commit a settled edit into the undo history (called on a debounce, and
    /// before an undo so an in-progress edit is not lost).
    fn commit_edit_if_pending(&mut self) {
        if self.graph != self.undo_baseline {
            self.undo_stack.push(self.undo_baseline.clone());
            if self.undo_stack.len() > 64 {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.undo_baseline = self.graph.clone();
        }
        self.edit_pending_since = None;
    }

    fn undo(&mut self) {
        self.commit_edit_if_pending();
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.graph.clone());
            self.graph = prev.clone();
            self.undo_baseline = prev;
            self.selected = None;
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.graph.clone());
            self.graph = next.clone();
            self.undo_baseline = next;
            self.selected = None;
        }
    }

    /// Begin gate auto-calibration: measure the noise floor, then the voice.
    fn start_calibration(&mut self) {
        self.calibration = Some(Calibration {
            phase: CalPhase::Quiet,
            since: std::time::Instant::now(),
            noise_sum: 0.0,
            noise_n: 0,
            voice_sum: 0.0,
            voice_n: 0,
        });
    }

    /// Advance calibration each frame, sampling the live input level.
    fn tick_calibration(&mut self) {
        let peak = self.engine.meters.in_peak();
        let Some(cal) = self.calibration.as_mut() else { return };
        match cal.phase {
            CalPhase::Quiet => {
                cal.noise_sum += peak;
                cal.noise_n += 1;
                if cal.since.elapsed().as_secs_f32() > 2.0 {
                    cal.phase = CalPhase::Talk;
                    cal.since = std::time::Instant::now();
                }
            }
            CalPhase::Talk => {
                cal.voice_sum += peak;
                cal.voice_n += 1;
                if cal.since.elapsed().as_secs_f32() > 2.5 {
                    self.finish_calibration();
                }
            }
        }
    }

    fn finish_calibration(&mut self) {
        let Some(cal) = self.calibration.take() else { return };
        let to_db = |lin: f32| 20.0 * lin.max(1e-5).log10();
        let noise_db = to_db(cal.noise_sum / cal.noise_n.max(1) as f32);
        let voice_db = to_db(cal.voice_sum / cal.voice_n.max(1) as f32);
        let margin = (0.5 * (voice_db - noise_db)).clamp(4.0, 10.0);
        let threshold = (noise_db + margin).clamp(-80.0, -6.0);

        let mut set = false;
        for node in &mut self.graph.nodes {
            if let NodeParams::Gate { threshold_db, .. } = &mut node.params {
                *threshold_db = threshold;
                set = true;
            }
        }
        self.status = if set {
            format!("gate calibrated: noise {noise_db:.0} dB, voice {voice_db:.0} dB -> threshold {threshold:.0} dB")
        } else {
            "no Gate node to calibrate (add one first)".into()
        };
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
        let preset = std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|s| Preset::from_ron(&s));
        self.status = match preset {
            Ok(mut p) => {
                // Never replace a preset the user already has.
                p.name = Preset::unused_name(&p.name);
                match p.save() {
                    Ok(_) => {
                        self.presets = Preset::load_all();
                        format!("imported '{}'", p.name)
                    }
                    Err(e) => format!("import failed: {e}"),
                }
            }
            Err(e) => format!("import failed: {e}"),
        };
    }
}

impl eframe::App for FormantApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint(); // live meters
        let ctx = ui.ctx().clone();

        // Undo/redo (Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z), unless typing in a field.
        if !ctx.egui_wants_keyboard_input() {
            let (undo, redo) = ctx.input(|i| {
                let cmd = i.modifiers.command;
                let undo = cmd && !i.modifiers.shift && i.key_pressed(egui::Key::Z);
                let redo = (cmd && i.modifiers.shift && i.key_pressed(egui::Key::Z))
                    || (cmd && i.key_pressed(egui::Key::Y));
                (undo, redo)
            });
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
        }

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
            let due = self.last_recovery.is_none_or(|t| t.elapsed().as_secs_f32() > 3.0);
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
            self.autostart_on = crate::platform::autostart_enabled();
        }
        self.last_tab = Some(self.tab);
        // Refresh the spectrum bands from the latest output samples.
        if self.engine.meters.copy_scope(&mut self.scope_buf) {
            self.spectrum.update(&self.scope_buf);
        }
        self.tick_calibration();
        egui::CentralPanel::default().show(ui, |ui| {
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

        // Undo history: commit a snapshot once edits settle (~0.5s of no change),
        // so a slider drag becomes one undo step rather than hundreds.
        if self.graph != self.undo_baseline {
            match self.edit_pending_since {
                None => self.edit_pending_since = Some(std::time::Instant::now()),
                Some(t) if t.elapsed().as_secs_f32() > 0.5 => self.commit_edit_if_pending(),
                _ => {}
            }
        } else {
            self.edit_pending_since = None;
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
        egui::Panel::top("top").show(ui, |ui| {
            let strip = ui.painter().add(egui::Shape::Noop);
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
                        ("BYPASSED", theme::EMBER)
                    } else {
                        ("LIVE", theme::GOOD)
                    };
                    if ui.button(RichText::new(txt).color(col).strong()).on_hover_text("Turn ALL processing off - passes your raw mic straight through.").clicked() {
                        self.engine.controls.toggle_bypass();
                    }
                });
            });
            ui.add_space(6.0);
            let rect = ui.min_rect().expand2(Vec2::new(8.0, 2.0));
            ui.painter().set(strip, hw::header_strip(rect));
       });
    }

    fn tab_mixer(&mut self, ui: &mut egui::Ui) {
        let (in_peak, out_peak, vad, gr, lufs) = {
            let m = &self.engine.meters;
            (m.in_peak(), m.out_peak(), m.vad(), m.gain_reduction_db(), m.lufs())
        };

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            // -- Master section: meter bridge, screen, transport ----------
            hw::rack_panel(ui, "MASTER", theme::CYAN, |ui| {
                ui.horizontal_top(|ui| {
                    let meter = Vec2::new(20.0, 150.0);
                    hw::led_ladder(ui, egui::Id::new("m-in"), "IN", in_peak, meter, "Incoming microphone level.");
                    hw::led_ladder(ui, egui::Id::new("m-out"), "OUT", out_peak, meter, "Processed level sent to your monitor and the virtual mic.");
                    hw::led_ladder(ui, egui::Id::new("m-vad"), "VOICE", vad, meter, "How confident the AI is that you're speaking right now.");
                    hw::led_ladder(
                        ui,
                        egui::Id::new("m-gr"),
                        "GR",
                        (gr / 20.0).clamp(0.0, 1.0),
                        meter,
                        "How much the compressor is currently turning the level down.",
                    );
                    ui.add_space(10.0);

                    let side = 214.0;
                    let screen_w = (ui.available_width() - side - 16.0).max(260.0);
                    let delay_ms = self.graph.latency_samples() as f32 / formant_core::SAMPLE_RATE as f32 * 1000.0;
                    let bands = self.spectrum.bands;
                    let description = format!(
                        "Output display. Loudness {}. Added delay {delay_ms:.0} milliseconds.",
                        if lufs <= -69.0 { "quiet".to_string() } else { format!("{lufs:.1} LUFS") }
                    );
                    hw::screen(ui, Vec2::new(screen_w, 168.0), &description, |p, rect| {
                        paint_spectrum(p, rect.shrink2(Vec2::new(8.0, 6.0)), &bands);
                        let loud = if lufs <= -69.0 { "  quiet".to_string() } else { format!("{lufs:>5.1} LUFS") };
                        let near = lufs > -20.0 && lufs < -12.0;
                        p.text(rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, "LOUDNESS", FontId::monospace(10.0), hw::READOUT.gamma_multiply(0.55));
                        p.text(
                            rect.left_top() + Vec2::new(10.0, 21.0),
                            Align2::LEFT_TOP,
                            loud,
                            FontId::monospace(17.0),
                            if near { theme::GOOD } else { hw::READOUT },
                        );
                        p.text(rect.right_top() + Vec2::new(-10.0, 8.0), Align2::RIGHT_TOP, "ADDED DELAY", FontId::monospace(10.0), hw::READOUT.gamma_multiply(0.55));
                        p.text(
                            rect.right_top() + Vec2::new(-10.0, 21.0),
                            Align2::RIGHT_TOP,
                            format!("{delay_ms:.0} ms"),
                            FontId::monospace(17.0),
                            if delay_ms < 30.0 { theme::GOOD } else { theme::EMBER },
                        );
                    })
                    .on_hover_text(
                        "Live spectrum of the processed output (low on the left). Loudness is momentary LUFS; \
                         many voice-chat and streaming targets sit around -16. Added delay is what your chain \
                         adds: under about 30 ms is comfortable to monitor live.",
                    );
                    ui.add_space(12.0);

                    ui.vertical(|ui| {
                        ui.set_width(side);
                        let bypassed = self.engine.controls.global_bypass();
                        let (label, col) = if bypassed { ("BYPASSED", theme::EMBER) } else { ("PROCESSING", theme::GOOD) };
                        if hw::led_toggle_named(ui, label, "Processing", !bypassed, col, side)
                            .on_hover_text("Turn ALL processing off - passes your raw mic straight through.")
                            .clicked()
                        {
                            self.engine.controls.toggle_bypass();
                        }
                        ui.add_space(6.0);
                        hw::silk_label(ui, "MIC MODE", 10.5);
                        let cur = self.engine.controls.mode();
                        let half = (side - ui.spacing().item_spacing.x) / 2.0;
                        for row in [[MuteMode::Vad, MuteMode::PushToTalk], [MuteMode::Toggle, MuteMode::AlwaysOpen]] {
                            ui.horizontal(|ui| {
                                for m in row {
                                    if hw::led_option(ui, m.label(), cur == m, theme::CYAN, half).on_hover_text(mode_help(m)).clicked() {
                                        self.engine.controls.set_mode(m);
                                    }
                                }
                            });
                        }
                        ui.add_space(6.0);
                        match &self.calibration {
                            None => {
                                if hw::led_button(ui, "Calibrate gate", false, theme::EMBER, side)
                                    .on_hover_text("Measures your background noise, then your voice, and sets the gate threshold between them. Follow the prompts.")
                                    .clicked()
                                {
                                    self.start_calibration();
                                }
                            }
                            Some(cal) => {
                                let msg = match cal.phase {
                                    CalPhase::Quiet => "Stay quiet...",
                                    CalPhase::Talk => "Now talk normally...",
                                };
                                if hw::led_button(ui, msg, true, theme::EMBER, side).on_hover_text("Click to cancel.").clicked() {
                                    self.calibration = None;
                                }
                            }
                        }
                    });
                });
            });

            // -- Presets ---------------------------------------------------
            ui.add_space(14.0);
            hw::rack_panel(ui, "PRESETS", theme::EMBER, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let presets = self.presets.clone();
                    if presets.is_empty() {
                        hw::silk_label(ui, "no presets yet - save one, or import in Setup", 11.0);
                    }
                    for preset in presets {
                        let w = (preset.name.len() as f32 * 7.4 + 36.0).max(90.0);
                        if hw::led_button(ui, &preset.name, false, theme::EMBER, w).clicked() {
                            self.graph = preset.graph.clone();
                            self.selected = None;
                            self.status = format!("loaded '{}'", preset.name);
                        }
                    }
                    ui.add_space(12.0);
                    ui.add(egui::TextEdit::singleline(&mut self.new_preset_name).hint_text("new preset name").desired_width(140.0));
                    if hw::led_button(ui, "Save", false, theme::GOOD, 70.0).clicked() && !self.new_preset_name.trim().is_empty() {
                        let preset = Preset::new(self.new_preset_name.trim(), self.graph.clone());
                        self.status = match preset.save() {
                            Ok(_) => format!("saved '{}'", preset.name),
                            Err(e) => format!("save failed: {e}"),
                        };
                        self.new_preset_name.clear();
                        self.presets = Preset::load_all();
                    }
                });
            });

            // -- Channel strip: one module per chain node, in signal order --
            ui.add_space(14.0);
            let ids: Vec<NodeId> = self
                .graph
                .exec_order()
                .into_iter()
                .filter(|id| !matches!(self.graph.kind_of(*id), Some(NodeKind::Input) | Some(NodeKind::Output)))
                .collect();
            hw::rack_panel(ui, "CHANNEL STRIP", theme::CYAN, |ui| {
                if ids.is_empty() {
                    hw::silk_label(ui, "empty chain - add nodes in the Nodes tab", 11.0);
                }
                egui::ScrollArea::horizontal().id_salt("strip").show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for id in ids {
                            self.channel(ui, id);
                        }
                    });
                    ui.add_space(6.0);
                });
            });
            ui.add_space(8.0);
        });
    }

    /// One channel module for a chain node: name plate (click to edit in
    /// Nodes), on and solo buttons, and its main control as a knob.
    fn channel(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(node) = self.graph.node(id).cloned() else { return };
        let accent = node_accent(node.params.kind());
        let label = node_label(&node.params).to_uppercase();
        ui.push_id(id, |ui| {
            hw::faceplate(ui, None, accent, |ui| {
                ui.set_width(104.0);
                ui.set_min_height(212.0);
                ui.vertical_centered(|ui| {
                    let name = ui
                        .add(egui::Label::new(RichText::new(&label).color(accent).strong().size(12.5)).sense(Sense::click()).truncate())
                        .on_hover_text(format!("{}\n\nClick to edit this node in the Nodes tab.", kind_help(node.params.kind())));
                    if name.clicked() {
                        self.selected = Some(id);
                        self.tab = Tab::Nodes;
                    }
                    ui.add_space(4.0);
                    if hw::led_toggle_named(ui, "ON", &format!("{} on", node_label(&node.params)), !node.bypass, theme::GOOD, 104.0).on_hover_text("Switch this node in or out of the chain.").clicked() {
                        if let Some(n) = self.graph.node_mut(id) {
                            n.bypass = !n.bypass;
                        }
                    }
                    if hw::led_toggle_named(ui, "SOLO", &format!("{} solo", node_label(&node.params)), node.solo, theme::EMBER, 104.0)
                        .on_hover_text("Hear only this node's output, to audition it in isolation.")
                        .clicked()
                    {
                        self.graph.set_solo(id, !node.solo);
                    }
                    ui.add_space(8.0);

                    let mut params = node.params.clone();
                    let mut changed = false;
                    match crate::controls::primary_knob(&mut params) {
                        Some((value, spec, help)) => {
                            changed |= hw::knob(ui, value, &spec, accent, 70.0).on_hover_text(help).changed();
                        }
                        None => {
                            ui.add_space(40.0);
                            let hint = if matches!(params, NodeParams::Vst3 { .. }) { "edit in Nodes" } else { "no controls" };
                            hw::silk_label(ui, hint, 10.5);
                        }
                    }
                    // The gate's lookahead is tuned often, so expose it here too.
                    if let NodeParams::Gate { lookahead_ms, .. } = &mut params {
                        let spec = hw::KnobSpec {
                            label: "look",
                            min: 0.0,
                            max: 40.0,
                            default: 10.0,
                            log: false,
                            bipolar: false,
                            unit: "ms",
                            decimals: 0,
                        };
                        changed |= hw::knob(ui, lookahead_ms, &spec, accent, 52.0)
                            .on_hover_text("Lookahead: opens the gate just before a word starts so onsets aren't clipped. 10-15 ms is a good balance.")
                            .changed();
                    }
                    if changed {
                        if let Some(n) = self.graph.node_mut(id) {
                            n.params = params;
                        }
                    }
                });
            });
        });
        ui.add_space(4.0);
    }

    fn tab_nodes(&mut self, ui: &mut egui::Ui) {
        // Inspector lives in a resizable right panel (drag its left edge); its
        // contents scroll so nothing clips.
        egui::Panel::right("inspector")
            .resizable(true)
            .default_size(280.0)
            .size_range(220.0..=520.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.draw_inspector(ui));
            });
        // The graph canvas fills whatever space is left.
        egui::CentralPanel::default().show(ui, |ui| {
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

        // Wires (under nodes), each with a midpoint handle to remove it.
        let mut wire_to_remove: Option<usize> = None;
        for (i, c) in self.graph.connections.clone().iter().enumerate() {
            let (Some(&(fp, ..)), Some(&(tp, ..))) = (layout.get(&c.from), layout.get(&c.to)) else {
                continue;
            };
            let a = fp + Vec2::new(ns.x, ns.y * 0.5);
            let b = tp + Vec2::new(0.0, ns.y * 0.5);
            let dx = ((b.x - a.x).abs() * 0.5).clamp(40.0, 160.0);
            let mid = cubic(a, Pos2::new(a.x + dx, a.y), Pos2::new(b.x - dx, b.y), b, 0.5);
            let handle = Rect::from_center_size(mid, Vec2::splat((18.0 * zoom).max(14.0)));
            let resp = ui
                .interact(handle, egui::Id::new(("fmt_wire", i)), Sense::click())
                .on_hover_text("Remove this connection");
            let hovered = resp.hovered();
            wire(&painter, a, b, if hovered { theme::EMBER } else { theme::CYAN.gamma_multiply(0.85) });
            if hovered {
                painter.circle_filled(mid, 8.0 * zoom.max(0.7), theme::BG);
                painter.text(mid, Align2::CENTER_CENTER, "x", FontId::proportional(14.0 * zoom.max(0.7)), theme::EMBER);
            } else {
                painter.circle_filled(mid, 2.5 * zoom, theme::CYAN.gamma_multiply(0.5));
            }
            if resp.clicked() {
                wire_to_remove = Some(i);
            }
        }
        if let Some(i) = wire_to_remove {
            if i < self.graph.connections.len() {
                self.graph.connections.remove(i);
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
            hw::module_box(&painter, rect, zoom, accent, selected, resp.hovered(), bypass);

            let title_col = if bypass { theme::MUTED } else { theme::TEXT };
            let label = labels.get(&id).cloned().unwrap_or_else(|| kind.label().to_string());
            hw::silk_text(&painter, rect.min + Vec2::new(13.0 * zoom, 16.0 * zoom), Align2::LEFT_TOP, &label, 15.0 * zoom, title_col);
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

            let hit = Vec2::splat(18.0 * zoom);
            if kind != NodeKind::Input {
                let p = rect.left_center();
                hw::jack(&painter, p, zoom, theme::EMBER);
                if ui.interact(Rect::from_center_size(p, hit), egui::Id::new(("fmt_in", id)), Sense::click()).clicked() {
                    self.graph.disconnect_into(id);
                }
            }
            if kind != NodeKind::Output {
                let p = rect.right_center();
                hw::jack(&painter, p, zoom, theme::CYAN);
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
            ui.horizontal(|ui| {
                let can_undo = !self.undo_stack.is_empty() || self.graph != self.undo_baseline;
                let can_redo = !self.redo_stack.is_empty();
                if ui.add_enabled(can_undo, egui::Button::new("Undo")).on_hover_text("Undo the last edit (Ctrl+Z).").clicked() {
                    self.undo();
                }
                if ui.add_enabled(can_redo, egui::Button::new("Redo")).on_hover_text("Redo (Ctrl+Y).").clicked() {
                    self.redo();
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
            let mut solo_clicked = false;
            ui.horizontal(|ui| {
                ui.checkbox(&mut on, "enabled");
                if ui
                    .selectable_label(node.solo, RichText::new("solo").color(if node.solo { theme::EMBER } else { theme::MUTED }))
                    .on_hover_text("Hear only this node's output, to audition it in isolation.")
                    .clicked()
                {
                    solo_clicked = true;
                }
            });

            let accent = node_accent(params.kind());
            if crate::controls::draw(ui, &mut params, accent).is_none() {
            match &mut params {
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
                    _ => {}
                }
            }

            let kind = params.kind();
            if let Some(n) = self.graph.node_mut(id) {
                n.params = params;
                n.bypass = !on;
            }
            if solo_clicked {
                self.graph.set_solo(id, !node.solo);
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
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            // -- Presets --------------------------------------------------
            hw::rack_panel(ui, "PRESETS", theme::EMBER, |ui| {
                ui.horizontal(|ui| {
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

            ui.add_space(14.0);
            hw::rack_panel(ui, "DEVICES", theme::CYAN, |ui| {
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
                        if ui.button("×").on_hover_text("Remove this output").clicked() {
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

            ui.add_space(14.0);
            hw::rack_panel(ui, "HOTKEYS", theme::CYAN, |ui| {
                ui.horizontal(|ui| {
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
                            hw::readout(ui, &name, 110.0);
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

            ui.add_space(14.0);
            hw::rack_panel(ui, "STARTUP", theme::CYAN, |ui| {
                let mut autostart = self.autostart_on;
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
                    self.autostart_on = crate::platform::autostart_enabled();
                }
                ui.label(
                    RichText::new("Closing the window hides Formant to the system tray; use the tray icon (or Quit) to bring it back.")
                        .color(theme::MUTED)
                        .small(),
                );
            });

            ui.add_space(14.0);
            hw::rack_panel(ui, "ABOUT", theme::CYAN, |ui| {
                ui.horizontal(|ui| {
                    hw::silk_label(ui, "VERSION", 10.5);
                    hw::readout(ui, env!("CARGO_PKG_VERSION"), 70.0);
                });
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
/// A pill-style tab button with a clear active state + accent underline.
fn tab_button(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(94.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, active, format!("{label} tab")));
    // egui has no tab widget type, so it reports a toggle button ("Mixer tab,
    // off"). Re-role it as a tab with a selected state: screen readers then say
    // "tab, selected", and selecting it (UIA SelectionItem) sends a click.
    ui.ctx().accesskit_node_builder(resp.id, |node| {
        node.set_role(egui::accesskit::Role::Tab);
        node.clear_toggled();
        node.set_selected(active);
    });
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(8),
            Stroke::new(2.0, Color32::from_rgb(120, 230, 255)),
            egui::StrokeKind::Outside,
        );
    }
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

/// The dark rack the panels sit in.
fn paint_backdrop(ui: &egui::Ui) {
    hw::rack_backdrop(ui.painter(), ui.max_rect());
}

/// Spectrum bars for the master screen, low frequencies on the left. Lit bars
/// glow; the unlit part of each column shows as a faint grid, like a real
/// segment display.
fn paint_spectrum(p: &egui::Painter, rect: Rect, bands: &[f32]) {
    let n = bands.len().max(1);
    let gap = 2.0;
    let bw = ((rect.width() - gap * (n as f32 - 1.0)) / n as f32).max(1.0);
    let top = rect.top() + 44.0; // leave room for the readouts
    let height = (rect.bottom() - top).max(10.0);
    for (i, &v) in bands.iter().enumerate() {
        let v = v.clamp(0.0, 1.0);
        let x = rect.left() + i as f32 * (bw + gap);
        let col = theme::lerp(theme::CYAN, theme::EMBER, v);
        let column = Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + bw, rect.bottom()));
        p.rect_filled(column, CornerRadius::same(1), theme::lerp(Color32::from_rgb(7, 10, 12), theme::CYAN, 0.05));
        let h = v * height;
        if h > 0.5 {
            let bar = Rect::from_min_max(Pos2::new(x, rect.bottom() - h), Pos2::new(x + bw, rect.bottom()));
            p.rect_filled(bar.expand(1.0), CornerRadius::same(1), col.gamma_multiply(0.2));
            p.rect_filled(bar, CornerRadius::same(1), col);
        }
    }
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
        NodeKind::PitchShift => "High-quality pitch and formant shifting. Move your pitch up or down, or change the apparent size of your voice (formant) on its own. Preserve formants keeps it natural (no chipmunk). This is the one to use for musical or natural shifts.",
        NodeKind::Pitch => "Warp: the original, lo-fi phase-vocoder shifter. Grainier and more 'effected' than the Pitch node, which makes it fun for robotic, alien, and glitchy character voices.",
        NodeKind::Autotune => "Real-time pitch correction: detects the note you're singing and pulls it to the nearest note in a key/scale. Set the key and scale, then dial strength (how hard) and speed (fast for the robotic effect, slow for natural).",
        NodeKind::Reverb => "Adds the sense of a room or space around your voice. Subtle amounts add depth; large amounts sound cavernous.",
        NodeKind::Delay => "An echo. Short times thicken, longer times repeat. Feedback controls how many repeats.",
        NodeKind::Chorus => "Thickens and widens your voice by mixing in a slightly detuned, wobbling copy.",
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
