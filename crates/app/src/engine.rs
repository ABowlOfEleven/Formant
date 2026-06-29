//! The audio engine wired for the UI: backend + chain callback + shared state.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::Result;
use formant_audio::hotkeys::{self, SharedBindings};
use formant_audio::{Stats, WasapiBackend};
use formant_core::backend::AudioBackend;
use formant_core::{AudioEffect, Config, Controls, Graph, GraphProcessor, Meters, MuteMode, NodeId};

/// Command sent from the UI thread to the audio thread to (un)install the effect
/// backing a `Vst3` node. Plugins are instantiated on the UI thread (slow load)
/// and handed across here, never blocking the audio callback.
enum EffectCommand {
    Install { id: NodeId, effect: Box<dyn AudioEffect> },
    Remove { id: NodeId },
    SetParam { id: NodeId, param: u32, value: f64 },
}

/// Owns the running audio backend and every handle the UI needs to observe or
/// drive it. Dropping it stops everything.
pub struct Engine {
    backend: WasapiBackend,
    pub controls: Arc<Controls>,
    pub meters: Arc<Meters>,
    pub bindings: Arc<SharedBindings>,
    pub stats: Arc<Stats>,
    pub mic_name: String,
    pub output_names: Vec<String>,
    graph: Arc<Mutex<Graph>>,
    graph_dirty: Arc<AtomicBool>,
    effect_tx: Sender<EffectCommand>,
    hotkey_running: Arc<AtomicBool>,
    hotkey_handle: Option<JoinHandle<()>>,
}

impl Engine {
    /// Resolve devices, start the duplex backend with the graph, and launch the
    /// hotkey listener.
    pub fn start(config: &Config, initial_graph: Graph) -> Result<Self> {
        let routing = formant_audio::resolve(&config.devices)?;
        let mic_name = routing.mic.name.clone();
        let output_names = routing.outputs.iter().map(|o| o.name.clone()).collect();
        let output_ids = routing.outputs.iter().map(|o| o.id.clone()).collect();

        let mut backend = WasapiBackend::new(routing.mic.id.clone(), output_ids);
        let stats = backend.stats();

        let controls = Controls::new(MuteMode::Vad);
        let meters = Arc::new(Meters::default());
        let graph = Arc::new(Mutex::new(initial_graph.clone()));
        let graph_dirty = Arc::new(AtomicBool::new(true));
        let bindings = SharedBindings::new(&config.bindings);

        let (effect_tx, effect_rx) = mpsc::channel::<EffectCommand>();

        let mut processor = GraphProcessor::new(&initial_graph);
        let cb_graph = Arc::clone(&graph);
        let cb_dirty = Arc::clone(&graph_dirty);
        let cb_controls = Arc::clone(&controls);
        let cb_meters = Arc::clone(&meters);
        backend.start(Box::new(move |input: &[f32], output: &mut [f32]| {
            // Drain pending VST install/remove handoffs.
            while let Ok(cmd) = effect_rx.try_recv() {
                match cmd {
                    EffectCommand::Install { id, effect } => processor.install_effect(id, effect),
                    EffectCommand::Remove { id } => processor.remove_effect(id),
                    EffectCommand::SetParam { id, param, value } => {
                        processor.set_effect_param(id, param, value)
                    }
                }
            }

            // Apply edited graph when flagged — never block the audio thread.
            if cb_dirty.swap(false, Ordering::Acquire) {
                match cb_graph.try_lock() {
                    Ok(g) => processor.apply_graph(&g),
                    Err(_) => cb_dirty.store(true, Ordering::Release), // retry next block
                }
            }

            if cb_controls.global_bypass() {
                output.copy_from_slice(input); // dry passthrough
            } else {
                processor.gate_override = cb_controls.gate_override();
                processor.process(input, output);
            }

            cb_meters.set_in_peak(peak(input));
            cb_meters.set_out_peak(peak(output));
            cb_meters.set_vad(processor.vad());
            cb_meters.set_gain_reduction_db(processor.gain_reduction_db());
        }))?;

        let hotkey_running = Arc::new(AtomicBool::new(true));
        let hotkey_handle = Some(hotkeys::spawn(
            Arc::clone(&controls),
            Arc::clone(&bindings),
            Arc::clone(&hotkey_running),
        ));

        Ok(Self {
            backend,
            controls,
            meters,
            bindings,
            stats,
            mic_name,
            output_names,
            graph,
            graph_dirty,
            effect_tx,
            hotkey_running,
            hotkey_handle,
        })
    }

    /// Push an edited graph to the audio thread.
    pub fn push_graph(&self, g: &Graph) {
        if let Ok(mut guard) = self.graph.lock() {
            *guard = g.clone();
            self.graph_dirty.store(true, Ordering::Release);
        }
    }

    /// Hand a freshly-instantiated VST effect to the audio thread for a node.
    pub fn install_effect(&self, id: NodeId, effect: Box<dyn AudioEffect>) {
        let _ = self.effect_tx.send(EffectCommand::Install { id, effect });
    }

    /// Tell the audio thread to drop the effect for a node.
    pub fn remove_effect(&self, id: NodeId) {
        let _ = self.effect_tx.send(EffectCommand::Remove { id });
    }

    /// Set a VST node's normalized parameter value.
    pub fn set_effect_param(&self, id: NodeId, param: u32, value: f64) {
        let _ = self.effect_tx.send(EffectCommand::SetParam { id, param, value });
    }

    pub fn stop(&mut self) {
        self.hotkey_running.store(false, Ordering::SeqCst);
        if let Some(h) = self.hotkey_handle.take() {
            let _ = h.join();
        }
        self.backend.stop();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn peak(buf: &[f32]) -> f32 {
    buf.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}
