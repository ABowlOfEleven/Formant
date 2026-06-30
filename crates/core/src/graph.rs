//! Node graph: a user-wired signal flow the audio engine executes.
//!
//! The model ([`Graph`], [`Node`], [`Connection`]) is serializable and is what
//! the UI edits and presets store. The runtime ([`GraphProcessor`]) holds the
//! stateful DSP for each node and executes the active path mic → … → output.
//!
//! Phase-2 v1 keeps it tractable: every node has one audio input and one audio
//! output, with a single `Input` and `Output` node. The active signal path is
//! found by walking upstream from `Output`; nodes off that path are inactive.
//! Multi-input mixing / parallel branches are a later step.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::dsp::{Biquad, Compressor, DeEsser, Denoise, Eq, Gate, Limiter, Saturator};
use crate::engine::GateOverride;
use crate::types::Sample;
use crate::SAMPLE_RATE;

pub type NodeId = u64;

/// VAD probability above which a VAD-gated gate opens.
const VAD_OPEN: f32 = 0.5;

/// A processor for a `Vst3` node, supplied by the host app (which owns the
/// plugin instance). Lives behind a trait so core never depends on the VST3 /
/// Windows hosting layer.
pub trait AudioEffect: Send {
    fn process(&mut self, input: &[Sample], output: &mut [Sample]);
    /// Set a normalized (0..1) plugin parameter. No-op for built-ins.
    fn set_param(&mut self, _id: u32, _value: f64) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Input,
    Output,
    HighPass,
    Denoise,
    Gate,
    DeEsser,
    Compressor,
    Eq,
    Saturator,
    Limiter,
    Gain,
    Makeup,
    Mix,
    Vst3,
}

impl NodeKind {
    /// Effect kinds the user can add (Input/Output are fixed).
    pub const EFFECTS: [NodeKind; 11] = [
        NodeKind::HighPass,
        NodeKind::Denoise,
        NodeKind::Gate,
        NodeKind::DeEsser,
        NodeKind::Compressor,
        NodeKind::Eq,
        NodeKind::Saturator,
        NodeKind::Limiter,
        NodeKind::Gain,
        NodeKind::Makeup,
        NodeKind::Mix,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Input => "Input",
            NodeKind::Output => "Output",
            NodeKind::HighPass => "High-pass",
            NodeKind::Denoise => "RNNoise",
            NodeKind::Gate => "Gate",
            NodeKind::DeEsser => "De-esser",
            NodeKind::Compressor => "Compressor",
            NodeKind::Eq => "EQ",
            NodeKind::Saturator => "Saturator",
            NodeKind::Limiter => "Limiter",
            NodeKind::Gain => "Gain",
            NodeKind::Makeup => "Makeup",
            NodeKind::Mix => "Mix",
            NodeKind::Vst3 => "VST3",
        }
    }

    pub fn default_params(self) -> NodeParams {
        match self {
            NodeKind::Input => NodeParams::Input,
            NodeKind::Output => NodeParams::Output,
            NodeKind::HighPass => NodeParams::HighPass { cutoff_hz: 80.0 },
            NodeKind::Denoise => NodeParams::Denoise,
            NodeKind::Gate => NodeParams::Gate {
                threshold_db: -45.0,
                range_db: -60.0,
                attack_ms: 2.0,
                hold_ms: 40.0,
                release_ms: 120.0,
                vad_gate: true,
            },
            NodeKind::DeEsser => NodeParams::DeEsser {
                threshold_db: -28.0,
                ratio: 4.0,
                split_hz: 6500.0,
            },
            NodeKind::Compressor => NodeParams::Compressor { threshold_db: -18.0, ratio: 3.0 },
            NodeKind::Eq => NodeParams::Eq { low_db: 0.0, mid_db: 0.0, high_db: 0.0 },
            NodeKind::Saturator => NodeParams::Saturator { drive: 2.0, mix: 0.3 },
            NodeKind::Limiter => NodeParams::Limiter { ceiling_db: -1.0 },
            NodeKind::Gain => NodeParams::Gain { gain_db: 0.0 },
            NodeKind::Makeup => NodeParams::Makeup { gain_db: 0.0 },
            NodeKind::Mix => NodeParams::Mix { gain_db: 0.0 },
            NodeKind::Vst3 => NodeParams::Vst3 {
                binary: String::new(),
                name: "VST3".into(),
                params: Vec::new(),
            },
        }
    }
}

/// Per-node parameters. The variant is the node's kind.
///
/// Not `Copy` — `Vst3` carries owned strings (the plugin path + name).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeParams {
    Input,
    Output,
    HighPass { cutoff_hz: f32 },
    Denoise,
    Gate {
        threshold_db: f32,
        range_db: f32,
        attack_ms: f32,
        hold_ms: f32,
        release_ms: f32,
        vad_gate: bool,
    },
    DeEsser { threshold_db: f32, ratio: f32, split_hz: f32 },
    Compressor { threshold_db: f32, ratio: f32 },
    Eq { low_db: f32, mid_db: f32, high_db: f32 },
    Saturator { drive: f32, mix: f32 },
    Limiter { ceiling_db: f32 },
    Gain { gain_db: f32 },
    Makeup { gain_db: f32 },
    /// Sums all of its inputs (parallel/blend routing), with an output trim.
    Mix { gain_db: f32 },
    /// A hosted VST3 plugin. `params` persists edited (id, normalized) values so
    /// presets remember the plugin's settings; re-applied on instantiation.
    Vst3 { binary: String, name: String, params: Vec<(u32, f64)> },
}

impl NodeParams {
    pub fn kind(&self) -> NodeKind {
        match self {
            NodeParams::Input => NodeKind::Input,
            NodeParams::Output => NodeKind::Output,
            NodeParams::HighPass { .. } => NodeKind::HighPass,
            NodeParams::Denoise => NodeKind::Denoise,
            NodeParams::Gate { .. } => NodeKind::Gate,
            NodeParams::DeEsser { .. } => NodeKind::DeEsser,
            NodeParams::Compressor { .. } => NodeKind::Compressor,
            NodeParams::Eq { .. } => NodeKind::Eq,
            NodeParams::Saturator { .. } => NodeKind::Saturator,
            NodeParams::Limiter { .. } => NodeKind::Limiter,
            NodeParams::Gain { .. } => NodeKind::Gain,
            NodeParams::Makeup { .. } => NodeKind::Makeup,
            NodeParams::Mix { .. } => NodeKind::Mix,
            NodeParams::Vst3 { .. } => NodeKind::Vst3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub params: NodeParams,
    pub bypass: bool,
    pub pos: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub from: NodeId,
    pub to: NodeId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub connections: Vec<Connection>,
    next_id: NodeId,
}

impl Default for Graph {
    fn default() -> Self {
        Self::default_chain()
    }
}

impl Graph {
    /// The familiar fixed chain as a wired graph: Input → HPF → RNNoise → Gate →
    /// De-ess → Comp → EQ → Makeup → Output, laid out left to right.
    pub fn default_chain() -> Self {
        let mut g = Graph { nodes: Vec::new(), connections: Vec::new(), next_id: 1 };
        let kinds = [
            NodeKind::Input,
            NodeKind::HighPass,
            NodeKind::Denoise,
            NodeKind::Gate,
            NodeKind::DeEsser,
            NodeKind::Compressor,
            NodeKind::Eq,
            NodeKind::Makeup,
            NodeKind::Output,
        ];
        let mut prev: Option<NodeId> = None;
        for (i, kind) in kinds.iter().enumerate() {
            let pos = [40.0 + i as f32 * 150.0, 120.0];
            let id = g.add_node(kind.default_params(), pos);
            if let Some(p) = prev {
                g.connect(p, id);
            }
            prev = Some(id);
        }
        g
    }

    pub fn add_node(&mut self, params: NodeParams, pos: [f32; 2]) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.push(Node { id, params, bypass: false, pos });
        id
    }

    pub fn remove_node(&mut self, id: NodeId) {
        // Input/Output are permanent.
        if matches!(self.kind_of(id), Some(NodeKind::Input) | Some(NodeKind::Output)) {
            return;
        }
        self.nodes.retain(|n| n.id != id);
        self.connections.retain(|c| c.from != id && c.to != id);
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    pub fn kind_of(&self, id: NodeId) -> Option<NodeKind> {
        self.node(id).map(|n| n.params.kind())
    }

    /// Connect `from`'s output to `to`'s input. Most nodes take a single source,
    /// so an existing connection into `to` is replaced; a `Mix` node accepts many
    /// inputs and sums them, so connections accumulate (deduped). Self-loops and
    /// feeding the Input node are rejected. A node's output may fan out to many
    /// destinations (that's how you "split" a signal — no dedicated node needed).
    pub fn connect(&mut self, from: NodeId, to: NodeId) {
        if from == to || self.kind_of(to) == Some(NodeKind::Input) {
            return;
        }
        if self.kind_of(to) == Some(NodeKind::Mix) {
            // Multi-input: keep prior sources, just avoid a duplicate edge.
            self.connections.retain(|c| !(c.from == from && c.to == to));
        } else {
            self.connections.retain(|c| c.to != to);
        }
        self.connections.push(Connection { from, to });
    }

    pub fn disconnect_into(&mut self, to: NodeId) {
        self.connections.retain(|c| c.to != to);
    }

    pub fn upstream(&self, to: NodeId) -> Option<NodeId> {
        self.connections.iter().find(|c| c.to == to).map(|c| c.from)
    }

    pub fn input_id(&self) -> Option<NodeId> {
        self.nodes.iter().find(|n| n.params.kind() == NodeKind::Input).map(|n| n.id)
    }

    pub fn output_id(&self) -> Option<NodeId> {
        self.nodes.iter().find(|n| n.params.kind() == NodeKind::Output).map(|n| n.id)
    }

    /// The active execution path, walking upstream from Output following the
    /// single-input chain. Useful for the linear channel-strip view; for the
    /// full DAG (parallel branches) use [`Graph::exec_order`].
    pub fn exec_path(&self) -> Vec<NodeId> {
        let Some(out) = self.output_id() else {
            return Vec::new();
        };
        let mut path = vec![out];
        let mut cur = out;
        while let Some(up) = self.upstream(cur) {
            if path.contains(&up) {
                break; // cycle guard
            }
            path.push(up);
            cur = up;
        }
        path.reverse();
        path
    }

    /// All direct inputs feeding each node (`to` → list of `from`s), in the
    /// order they were connected. Nodes with no inputs are absent.
    pub fn upstreams_map(&self) -> HashMap<NodeId, Vec<NodeId>> {
        let mut m: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for c in &self.connections {
            m.entry(c.to).or_default().push(c.from);
        }
        m
    }

    /// A topological execution order over *all* nodes (Kahn's algorithm), so a
    /// node is always processed after every node that feeds it. Handles parallel
    /// branches and multi-input `Mix` nodes; any nodes in a cycle are appended
    /// last (defensive — the editor shouldn't create cycles).
    pub fn exec_order(&self) -> Vec<NodeId> {
        let mut indeg: HashMap<NodeId, usize> =
            self.nodes.iter().map(|n| (n.id, 0usize)).collect();
        for c in &self.connections {
            if let Some(d) = indeg.get_mut(&c.to) {
                *d += 1;
            }
        }
        // Seed with zero-in-degree nodes, preserving node declaration order.
        let mut queue: Vec<NodeId> =
            self.nodes.iter().filter(|n| indeg[&n.id] == 0).map(|n| n.id).collect();
        let mut order = Vec::with_capacity(self.nodes.len());
        let mut qi = 0;
        while qi < queue.len() {
            let n = queue[qi];
            qi += 1;
            order.push(n);
            for c in &self.connections {
                if c.from == n {
                    if let Some(d) = indeg.get_mut(&c.to) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(c.to);
                        }
                    }
                }
            }
        }
        if order.len() < self.nodes.len() {
            for n in &self.nodes {
                if !order.contains(&n.id) {
                    order.push(n.id);
                }
            }
        }
        order
    }

    /// A showcase chain using parallel routing: the cleaned voice is split into a
    /// dry path, a heavily-compressed parallel path, and a saturated path, all
    /// blended back together through a `Mix`, then EQ'd and limited. Demonstrates
    /// parallel compression, blend-in saturation, and wet/dry balance.
    pub fn parallel_demo() -> Self {
        use NodeParams as P;
        let mut g = Graph { nodes: Vec::new(), connections: Vec::new(), next_id: 1 };
        // Front-end cleanup (linear).
        let input = g.add_node(P::Input, [40.0, 240.0]);
        let hpf = g.add_node(P::HighPass { cutoff_hz: 85.0 }, [190.0, 240.0]);
        let denoise = g.add_node(P::Denoise, [340.0, 240.0]);
        let gate = g.add_node(
            P::Gate {
                threshold_db: -42.0,
                range_db: -60.0,
                attack_ms: 2.0,
                hold_ms: 40.0,
                release_ms: 120.0,
                vad_gate: true,
            },
            [490.0, 240.0],
        );
        // Three parallel branches off the cleaned signal.
        let dry = g.add_node(P::Gain { gain_db: -1.5 }, [690.0, 90.0]);
        let comp = g.add_node(P::Compressor { threshold_db: -34.0, ratio: 8.0 }, [690.0, 240.0]);
        let comp_lvl = g.add_node(P::Gain { gain_db: -7.0 }, [860.0, 240.0]);
        let sat = g.add_node(P::Saturator { drive: 4.0, mix: 1.0 }, [690.0, 380.0]);
        let sat_lvl = g.add_node(P::Gain { gain_db: -12.0 }, [860.0, 380.0]);
        // Blend the branches, then finish.
        let mix = g.add_node(P::Mix { gain_db: 0.0 }, [1040.0, 240.0]);
        let eq = g.add_node(P::Eq { low_db: 1.0, mid_db: 1.5, high_db: 2.0 }, [1210.0, 240.0]);
        let limiter = g.add_node(P::Limiter { ceiling_db: -1.0 }, [1380.0, 240.0]);
        let output = g.add_node(P::Output, [1550.0, 240.0]);

        g.connect(input, hpf);
        g.connect(hpf, denoise);
        g.connect(denoise, gate);
        // Split the gate output three ways.
        g.connect(gate, dry);
        g.connect(gate, comp);
        g.connect(gate, sat);
        g.connect(comp, comp_lvl);
        g.connect(sat, sat_lvl);
        // Sum the branches.
        g.connect(dry, mix);
        g.connect(comp_lvl, mix);
        g.connect(sat_lvl, mix);
        g.connect(mix, eq);
        g.connect(eq, limiter);
        g.connect(limiter, output);
        g
    }
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// Stateful DSP for one node.
enum NodeProc {
    Passthrough,
    HighPass(Biquad),
    Denoise(Box<Denoise>),
    Gate { gate: Gate, vad_gate: bool },
    DeEsser(DeEsser),
    Compressor(Compressor),
    Eq(Eq),
    Saturator(Saturator),
    Limiter(Limiter),
    Gain(f32),
    Makeup(f32),
    /// Output trim applied after the executor has summed the node's inputs.
    Mix(f32),
}

impl NodeProc {
    fn new(params: &NodeParams) -> Self {
        match *params {
            // Input/Output are identity; Vst3 is handled by the effects map, not
            // a NodeProc — Passthrough is just a placeholder for those.
            NodeParams::Input | NodeParams::Output | NodeParams::Vst3 { .. } => {
                NodeProc::Passthrough
            }
            NodeParams::HighPass { cutoff_hz } => {
                NodeProc::HighPass(Biquad::highpass(SAMPLE_RATE, cutoff_hz, 0.707))
            }
            NodeParams::Denoise => NodeProc::Denoise(Box::new(Denoise::new())),
            NodeParams::Gate { threshold_db, range_db, attack_ms, hold_ms, release_ms, vad_gate } => {
                NodeProc::Gate {
                    gate: Gate::new(SAMPLE_RATE, threshold_db, range_db, attack_ms, hold_ms, release_ms),
                    vad_gate,
                }
            }
            NodeParams::DeEsser { threshold_db, ratio, split_hz } => {
                NodeProc::DeEsser(DeEsser::new(SAMPLE_RATE, split_hz, threshold_db, ratio))
            }
            NodeParams::Compressor { threshold_db, ratio } => {
                NodeProc::Compressor(Compressor::new(SAMPLE_RATE, threshold_db, ratio, 10.0, 80.0, 0.0))
            }
            NodeParams::Eq { low_db, mid_db, high_db } => {
                NodeProc::Eq(Eq::new(SAMPLE_RATE, low_db, mid_db, high_db))
            }
            NodeParams::Saturator { drive, mix } => NodeProc::Saturator(Saturator::new(drive, mix)),
            NodeParams::Limiter { ceiling_db } => {
                NodeProc::Limiter(Limiter::new(SAMPLE_RATE, ceiling_db))
            }
            NodeParams::Gain { gain_db } => NodeProc::Gain(crate::dsp::db_to_lin(gain_db)),
            NodeParams::Makeup { gain_db } => NodeProc::Makeup(crate::dsp::db_to_lin(gain_db)),
            NodeParams::Mix { gain_db } => NodeProc::Mix(crate::dsp::db_to_lin(gain_db)),
        }
    }

    fn kind(&self) -> NodeKind {
        match self {
            NodeProc::Passthrough => NodeKind::Input, // distinguished by graph, not here
            NodeProc::HighPass(_) => NodeKind::HighPass,
            NodeProc::Denoise(_) => NodeKind::Denoise,
            NodeProc::Gate { .. } => NodeKind::Gate,
            NodeProc::DeEsser(_) => NodeKind::DeEsser,
            NodeProc::Compressor(_) => NodeKind::Compressor,
            NodeProc::Eq(_) => NodeKind::Eq,
            NodeProc::Saturator(_) => NodeKind::Saturator,
            NodeProc::Limiter(_) => NodeKind::Limiter,
            NodeProc::Gain(_) => NodeKind::Gain,
            NodeProc::Makeup(_) => NodeKind::Makeup,
            NodeProc::Mix(_) => NodeKind::Mix,
        }
    }

    /// Update params in place where possible (preserving filter state).
    fn set_params(&mut self, params: &NodeParams) {
        match (self, params) {
            (NodeProc::HighPass(b), NodeParams::HighPass { cutoff_hz }) => {
                *b = Biquad::highpass(SAMPLE_RATE, *cutoff_hz, 0.707);
            }
            (
                NodeProc::Gate { gate, vad_gate },
                NodeParams::Gate { threshold_db, range_db, attack_ms, hold_ms, release_ms, vad_gate: vg },
            ) => {
                gate.configure(*threshold_db, *range_db, *attack_ms, *hold_ms, *release_ms);
                *vad_gate = *vg;
            }
            (NodeProc::DeEsser(d), NodeParams::DeEsser { threshold_db, ratio, split_hz }) => {
                d.configure(*split_hz, *threshold_db, *ratio);
            }
            (NodeProc::Compressor(c), NodeParams::Compressor { threshold_db, ratio }) => {
                c.set_threshold_db(*threshold_db);
                c.set_ratio(*ratio);
            }
            (NodeProc::Eq(eq), NodeParams::Eq { low_db, mid_db, high_db }) => {
                eq.set_gains(*low_db, *mid_db, *high_db);
            }
            (NodeProc::Saturator(s), NodeParams::Saturator { drive, mix }) => {
                s.configure(*drive, *mix);
            }
            (NodeProc::Limiter(l), NodeParams::Limiter { ceiling_db }) => {
                l.configure(*ceiling_db);
            }
            (NodeProc::Gain(g), NodeParams::Gain { gain_db }) => {
                *g = crate::dsp::db_to_lin(*gain_db);
            }
            (NodeProc::Makeup(g), NodeParams::Makeup { gain_db }) => {
                *g = crate::dsp::db_to_lin(*gain_db);
            }
            (NodeProc::Mix(g), NodeParams::Mix { gain_db }) => {
                *g = crate::dsp::db_to_lin(*gain_db);
            }
            _ => {}
        }
    }

    fn process(
        &mut self,
        input: &[Sample],
        output: &mut [Sample],
        vad: &mut f32,
        gr_db: &mut f32,
        gate_override: GateOverride,
    ) {
        match self {
            NodeProc::Passthrough => output.copy_from_slice(input),
            NodeProc::HighPass(b) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = b.process(x);
                }
            }
            NodeProc::Denoise(d) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = d.process(x);
                }
                *vad = d.vad();
            }
            NodeProc::Gate { gate, vad_gate } => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = match gate_override {
                        GateOverride::ForceOpen => gate.process_gated(x, true),
                        GateOverride::ForceClosed => gate.process_gated(x, false),
                        GateOverride::Auto => {
                            if *vad_gate {
                                gate.process_gated(x, *vad > VAD_OPEN)
                            } else {
                                gate.process(x)
                            }
                        }
                    };
                }
            }
            NodeProc::DeEsser(d) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = d.process(x);
                }
            }
            NodeProc::Compressor(c) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = c.process(x);
                }
                *gr_db = c.gain_reduction_db();
            }
            NodeProc::Eq(eq) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = eq.process(x);
                }
            }
            NodeProc::Saturator(s) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = s.process(x);
                }
            }
            NodeProc::Limiter(l) => {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = l.process(x);
                }
            }
            NodeProc::Gain(g) | NodeProc::Makeup(g) | NodeProc::Mix(g) => {
                // For Mix, `input` is the executor's sum of the node's inputs;
                // `g` is the output trim.
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = x * *g;
                }
            }
        }
    }
}

/// Executes a [`Graph`] on the audio thread.
pub struct GraphProcessor {
    procs: HashMap<NodeId, NodeProc>,
    /// Host-supplied effects for `Vst3` nodes (installed via the handoff).
    effects: HashMap<NodeId, Box<dyn AudioEffect>>,
    kinds: HashMap<NodeId, NodeKind>,
    /// Topological execution order over all nodes.
    order: Vec<NodeId>,
    /// Direct inputs feeding each node (summed when more than one).
    upstreams: HashMap<NodeId, Vec<NodeId>>,
    /// The Output node, if any.
    output: Option<NodeId>,
    /// Reusable per-node output buffers, so the DAG can fan out and merge.
    node_bufs: HashMap<NodeId, Vec<Sample>>,
    bypass: HashMap<NodeId, bool>,
    a: Vec<Sample>,
    b: Vec<Sample>,
    last_vad: f32,
    last_gr_db: f32,
    pub gate_override: GateOverride,
}

impl GraphProcessor {
    pub fn new(graph: &Graph) -> Self {
        let mut p = Self {
            procs: HashMap::new(),
            effects: HashMap::new(),
            kinds: HashMap::new(),
            order: Vec::new(),
            upstreams: HashMap::new(),
            output: None,
            node_bufs: HashMap::new(),
            bypass: HashMap::new(),
            a: Vec::new(),
            b: Vec::new(),
            last_vad: 0.0,
            last_gr_db: 0.0,
            gate_override: GateOverride::Auto,
        };
        p.apply_graph(graph);
        p
    }

    /// Install (or replace) the effect backing a `Vst3` node. Called from the
    /// audio thread after the host hands an instance across.
    pub fn install_effect(&mut self, id: NodeId, effect: Box<dyn AudioEffect>) {
        self.effects.insert(id, effect);
    }

    pub fn remove_effect(&mut self, id: NodeId) {
        self.effects.remove(&id);
    }

    /// Route a parameter edit to a node's effect.
    pub fn set_effect_param(&mut self, node: NodeId, param: u32, value: f64) {
        if let Some(effect) = self.effects.get_mut(&node) {
            effect.set_param(param, value);
        }
    }

    /// Reconcile the runtime with an edited graph: add/remove node processors,
    /// update params in place (preserving state), and recompute the path.
    pub fn apply_graph(&mut self, graph: &Graph) {
        // Drop processors/effects/buffers for removed nodes.
        self.procs.retain(|id, _| graph.node(*id).is_some());
        self.effects.retain(|id, _| graph.node(*id).is_some());
        self.kinds.retain(|id, _| graph.node(*id).is_some());
        self.node_bufs.retain(|id, _| graph.node(*id).is_some());

        for node in &graph.nodes {
            self.bypass.insert(node.id, node.bypass);
            self.kinds.insert(node.id, node.params.kind());
            // Vst3 nodes are driven by the effects map, not a NodeProc.
            if node.params.kind() == NodeKind::Vst3 {
                continue;
            }
            match self.procs.get_mut(&node.id) {
                Some(proc) if same_kind(proc, node) => proc.set_params(&node.params),
                _ => {
                    self.procs.insert(node.id, NodeProc::new(&node.params));
                }
            }
        }
        self.order = graph.exec_order();
        self.upstreams = graph.upstreams_map();
        self.output = graph.output_id();
    }

    pub fn vad(&self) -> f32 {
        self.last_vad
    }

    pub fn gain_reduction_db(&self) -> f32 {
        self.last_gr_db
    }

    pub fn process(&mut self, input: &[Sample], output: &mut [Sample]) {
        let n = input.len();
        if self.order.is_empty() {
            output.copy_from_slice(input);
            return;
        }
        // One reusable output buffer per node, plus the gather/scratch pair.
        for id in &self.order {
            self.node_bufs.entry(*id).or_default().resize(n, 0.0);
        }
        self.a.resize(n, 0.0);
        self.b.resize(n, 0.0);

        // Take the proc/effect maps out to satisfy the borrow checker while we
        // also touch a/b/last_vad. Cheap (pointer swap), no allocation.
        let mut procs = std::mem::take(&mut self.procs);
        let mut effects = std::mem::take(&mut self.effects);

        for idx in 0..self.order.len() {
            let id = self.order[idx];
            let kind = self.kinds.get(&id).copied().unwrap_or(NodeKind::Output);

            // ── Gather this node's input into `a` ──
            if kind == NodeKind::Input {
                self.a[..n].copy_from_slice(input);
            } else {
                match self.upstreams.get(&id) {
                    Some(ups) if !ups.is_empty() => {
                        self.a[..n].copy_from_slice(&self.node_bufs[&ups[0]][..n]);
                        for u in &ups[1..] {
                            let src = &self.node_bufs[u];
                            for i in 0..n {
                                self.a[i] += src[i];
                            }
                        }
                    }
                    _ => self.a[..n].fill(0.0),
                }
            }

            // ── Process `a` → `b` ──
            let bypassed = self.bypass.get(&id).copied().unwrap_or(false);
            if bypassed || kind == NodeKind::Input || kind == NodeKind::Output {
                self.b[..n].copy_from_slice(&self.a[..n]);
            } else if kind == NodeKind::Vst3 {
                match effects.get_mut(&id) {
                    Some(effect) => effect.process(&self.a[..n], &mut self.b[..n]),
                    None => self.b[..n].copy_from_slice(&self.a[..n]),
                }
            } else if let Some(proc) = procs.get_mut(&id) {
                proc.process(
                    &self.a[..n],
                    &mut self.b[..n],
                    &mut self.last_vad,
                    &mut self.last_gr_db,
                    self.gate_override,
                );
            } else {
                self.b[..n].copy_from_slice(&self.a[..n]);
            }

            // ── Store `b` as this node's output ──
            self.node_bufs.get_mut(&id).unwrap()[..n].copy_from_slice(&self.b[..n]);
        }
        self.procs = procs;
        self.effects = effects;

        // Final output: the Output node's buffer, or the raw input passed through
        // if the Output is unwired (so disconnecting it never goes silent).
        match self.output {
            Some(out) if self.upstreams.get(&out).is_some_and(|u| !u.is_empty()) => {
                output.copy_from_slice(&self.node_bufs[&out][..n]);
            }
            _ => output.copy_from_slice(input),
        }
    }
}

fn same_kind(proc: &NodeProc, node: &Node) -> bool {
    let k = node.params.kind();
    if matches!(k, NodeKind::Input | NodeKind::Output) {
        matches!(proc, NodeProc::Passthrough)
    } else {
        proc.kind() == k
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_chain_path_is_linear_input_to_output() {
        let g = Graph::default_chain();
        let path = g.exec_path();
        assert_eq!(g.kind_of(path[0]), Some(NodeKind::Input));
        assert_eq!(g.kind_of(*path.last().unwrap()), Some(NodeKind::Output));
        assert_eq!(path.len(), 9);
    }

    #[test]
    fn processes_without_panicking_and_changes_signal() {
        let g = Graph::default_chain();
        let mut proc = GraphProcessor::new(&g);
        let input: Vec<f32> = (0..480).map(|n| 0.3 + 0.4 * (n as f32 * 0.05).sin()).collect();
        let mut out = vec![0.0; 480];
        proc.process(&input, &mut out);
        // HPF removes the DC offset -> mean moves toward zero.
        let in_mean: f32 = input.iter().sum::<f32>() / 480.0;
        let out_mean: f32 = out.iter().sum::<f32>() / 480.0;
        assert!(out_mean.abs() < in_mean.abs());
    }

    #[test]
    fn disconnected_output_passes_through() {
        let mut g = Graph::default_chain();
        let out = g.output_id().unwrap();
        g.disconnect_into(out);
        let mut proc = GraphProcessor::new(&g);
        let input = vec![0.5; 256];
        let mut output = vec![0.0; 256];
        proc.process(&input, &mut output);
        assert_eq!(input, output, "unwired output should pass the signal through");
    }

    #[test]
    fn vst3_node_routes_through_installed_effect() {
        // Input -> Vst3 -> Output
        let mut g = Graph { nodes: Vec::new(), connections: Vec::new(), next_id: 1 };
        let inp = g.add_node(NodeParams::Input, [0.0, 0.0]);
        let vst = g.add_node(
            NodeParams::Vst3 { binary: "x".into(), name: "Test".into(), params: Vec::new() },
            [1.0, 0.0],
        );
        let out = g.add_node(NodeParams::Output, [2.0, 0.0]);
        g.connect(inp, vst);
        g.connect(vst, out);

        let mut proc = GraphProcessor::new(&g);
        let input = vec![0.25_f32; 64];
        let mut output = vec![0.0_f32; 64];

        // No effect installed yet -> the VST node passes through.
        proc.process(&input, &mut output);
        assert_eq!(output, input);

        // Install a doubling effect and confirm it's applied.
        struct Double;
        impl AudioEffect for Double {
            fn process(&mut self, input: &[f32], output: &mut [f32]) {
                for (o, &x) in output.iter_mut().zip(input) {
                    *o = x * 2.0;
                }
            }
        }
        proc.install_effect(vst, Box::new(Double));
        proc.process(&input, &mut output);
        assert!(output.iter().all(|&x| (x - 0.5).abs() < 1e-6), "effect not applied");

        // Remove it -> passthrough again.
        proc.remove_effect(vst);
        proc.process(&input, &mut output);
        assert_eq!(output, input);
    }

    #[test]
    fn mix_node_sums_fanned_out_inputs() {
        // Input fans out to two unity-gain branches that recombine in a Mix.
        let mut g = Graph { nodes: Vec::new(), connections: Vec::new(), next_id: 1 };
        let inp = g.add_node(NodeParams::Input, [0.0, 0.0]);
        let a = g.add_node(NodeParams::Gain { gain_db: 0.0 }, [1.0, 0.0]);
        let b = g.add_node(NodeParams::Gain { gain_db: 0.0 }, [1.0, 1.0]);
        let mix = g.add_node(NodeParams::Mix { gain_db: 0.0 }, [2.0, 0.0]);
        let out = g.add_node(NodeParams::Output, [3.0, 0.0]);
        g.connect(inp, a);
        g.connect(inp, b);
        g.connect(a, mix);
        g.connect(b, mix);
        g.connect(mix, out);
        // The Mix should accept both inputs (multi-input), unlike a normal node.
        assert_eq!(g.upstreams_map().get(&mix).map(|v| v.len()), Some(2));

        let mut proc = GraphProcessor::new(&g);
        let input = vec![0.25_f32; 64];
        let mut output = vec![0.0_f32; 64];
        proc.process(&input, &mut output);
        assert!(output.iter().all(|&x| (x - 0.5).abs() < 1e-6), "Mix should sum the two branches");
    }

    #[test]
    fn parallel_demo_processes_cleanly() {
        let g = Graph::parallel_demo();
        // Topological order must cover every node and never be empty.
        assert_eq!(g.exec_order().len(), g.nodes.len());
        let mut proc = GraphProcessor::new(&g);
        let input: Vec<f32> = (0..480).map(|n| 0.4 * (n as f32 * 0.05).sin()).collect();
        let mut out = vec![0.0; 480];
        proc.process(&input, &mut out);
        assert!(out.iter().all(|x| x.is_finite()), "no NaN/inf from the parallel chain");
    }

    #[test]
    fn removing_node_drops_its_connections() {
        let mut g = Graph::default_chain();
        let gate = g.nodes.iter().find(|n| n.params.kind() == NodeKind::Gate).unwrap().id;
        g.remove_node(gate);
        assert!(g.node(gate).is_none());
        assert!(g.connections.iter().all(|c| c.from != gate && c.to != gate));
    }
}
