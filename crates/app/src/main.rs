//! Formant - creator-grade, Rust-native vocal processing.
//!
//! Launches the themed UI by default. `--seconds N` runs headless for testing.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod logging;
mod platform;
mod spectrum;
mod theme;
mod tray;
mod ui;
mod vst;

use std::time::Duration;

use formant_audio::com::ComGuard;
use formant_core::{Config, Graph, NodeParams};

use crate::engine::Engine;

fn main() -> anyhow::Result<()> {
    if let Some(name) = parse_flag("--vst") {
        return run_vst_test(&name, parse_seconds().unwrap_or(4));
    }
    if let Some(secs) = parse_seconds() {
        return run_headless(secs);
    }
    run_gui()
}

/// Headless check of the full path: insert a real VST before Output and run the
/// live mic through it (emits audio). Verifies the handoff + RT processing.
fn run_vst_test(name: &str, secs: u64) -> anyhow::Result<()> {
    let _com = ComGuard::new()?;
    let config = Config::load_or_default();

    let plugins = formant_vst3::scan();
    let plugin = plugins
        .iter()
        .filter(|p| p.is_effect())
        .find(|p| p.name.to_lowercase().contains(&name.to_lowercase()))
        .ok_or_else(|| anyhow::anyhow!("no effect matching {name:?}"))?;
    println!("inserting VST: {}", plugin.name);

    // Splice a VST node between the default chain's last effect and Output.
    let mut graph = Graph::default_chain();
    let out = graph.output_id().unwrap();
    let upstream = graph.upstream(out).unwrap();
    let pos = [graph.node(upstream).unwrap().pos[0] + 150.0, 120.0];
    let vst = graph.add_node(
        NodeParams::Vst3 {
            binary: plugin.binary.to_string_lossy().into_owned(),
            name: plugin.name.clone(),
            params: Vec::new(),
        },
        pos,
    );
    graph.connect(upstream, vst);
    graph.connect(vst, out);

    let engine = Engine::start(&config, graph)?;
    let (effect, editor) = crate::vst::VstEffect::load(&plugin.binary)?;
    println!("plugin exposes {} parameters", editor.params().len());
    engine.install_effect(vst, Box::new(effect));
    drop(editor); // headless: no GUI

    println!("running mic -> chain -> {} -> outputs for {secs}s...", plugin.name);
    std::thread::sleep(Duration::from_secs(secs));

    println!(
        "stopped. captured {} / rendered {} / underflows {}",
        engine.stats.captured_frames.load(std::sync::atomic::Ordering::Relaxed),
        engine.stats.rendered_frames.load(std::sync::atomic::Ordering::Relaxed),
        engine.stats.underflows.load(std::sync::atomic::Ordering::Relaxed),
    );
    Ok(())
}

fn run_gui() -> anyhow::Result<()> {
    if !platform::is_first_instance() {
        eprintln!("Formant is already running.");
        return Ok(());
    }
    logging::init();
    // COM for this (UI) thread - STA so it coexists with winit's OleInitialize.
    let com = ComGuard::new_sta()?;
    // If a config already exists, this is an upgrade, not a first run, so skip
    // the one-time welcome and tray hint.
    let config_existed = Config::default_path().is_some_and(|p| p.exists());
    let mut config = Config::load_or_default();
    if config_existed && !config.seen_welcome {
        config.seen_welcome = true;
        config.seen_tray_hint = true;
        let _ = config.save();
    }
    // Seed the bundled example presets on first run.
    formant_core::Preset::install_factory();
    // Restore the last session's graph, or start from the default chain.
    let graph = formant_core::session::load().unwrap_or_else(Graph::default_chain);
    let engine = Engine::start(&config, graph.clone())?;

    let icon = eframe::egui::IconData {
        rgba: include_bytes!("../icon.rgba").to_vec(),
        width: 256,
        height: 256,
    };
    let native = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([940.0, 660.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("Formant")
            .with_icon(icon),
        ..Default::default()
    };

    eframe::run_native(
        "Formant",
        native,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(ui::FormantApp::new(config, engine, com, graph)))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))
}

fn run_headless(secs: u64) -> anyhow::Result<()> {
    let _com = ComGuard::new()?;
    let config = Config::load_or_default();
    let engine = Engine::start(&config, Graph::default_chain())?;

    println!("Formant {} (headless)", env!("CARGO_PKG_VERSION"));
    println!("mic:    {}", engine.mic_name);
    for out in &engine.output_names {
        println!("output: {out}");
    }
    println!("running for {secs}s...");
    std::thread::sleep(Duration::from_secs(secs));

    println!(
        "stopped. captured {} / rendered {} / underflows {}",
        engine.stats.captured_frames.load(std::sync::atomic::Ordering::Relaxed),
        engine.stats.rendered_frames.load(std::sync::atomic::Ordering::Relaxed),
        engine.stats.underflows.load(std::sync::atomic::Ordering::Relaxed),
    );
    // engine drops here -> stops backend + hotkeys.
    Ok(())
}

fn parse_seconds() -> Option<u64> {
    parse_flag("--seconds").or_else(|| parse_flag("-s")).and_then(|v| v.parse().ok())
}

fn parse_flag(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}
