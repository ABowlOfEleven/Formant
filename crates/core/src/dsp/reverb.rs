//! Reverb: a Freeverb-style room and a Dattorro plate, with pre-delay,
//! modulation and wet filters.
//!
//! References (docs/research/effects-references.md, section 7): Freeverb by
//! Jezar at Dreampoint (public domain) for the room; J. Dattorro, "Effect
//! Design Part 1: Reverberator and Other Filters" (JAES 1997) for the plate
//! topology. The plate's delay lengths and output taps below are the values
//! in common use in open implementations of that paper; they were not
//! re-checked against the PDF itself.

use serde::{Deserialize, Serialize};

use crate::dsp::smooth::{Smoothed, GLIDE_MS};
use crate::dsp::svf::{Svf, SvfShape};
use crate::types::Sample;

/// Which reverb algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ReverbKind {
    /// Freeverb: parallel damped combs into series allpasses. A room.
    #[default]
    Room,
    /// Dattorro's plate: dense, bright, fast build-up (EMT 140 style).
    Plate,
}

impl ReverbKind {
    pub const ALL: [ReverbKind; 2] = [ReverbKind::Room, ReverbKind::Plate];
    pub fn label(self) -> &'static str {
        match self {
            ReverbKind::Room => "Room",
            ReverbKind::Plate => "Plate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReverbSettings {
    /// 0 to 1: tail length.
    pub room_size: f32,
    /// 0 to 1: how quickly the highs fade in the tail.
    pub damping: f32,
    pub mix: f32,
    pub kind: ReverbKind,
    pub predelay_ms: f32,
    /// Wet high-pass, Hz (0 = off).
    pub lowcut_hz: f32,
    /// Wet low-pass, Hz (20 kHz and up = off).
    pub highcut_hz: f32,
    /// 0 to 1: depth of the tail modulation (plate).
    pub modulation: f32,
}

impl Default for ReverbSettings {
    fn default() -> Self {
        Self {
            room_size: 0.5,
            damping: 0.5,
            mix: 0.2,
            kind: ReverbKind::Plate,
            predelay_ms: 20.0,
            lowcut_hz: 200.0,
            highcut_hz: 9000.0,
            modulation: 0.3,
        }
    }
}

const MAX_PREDELAY_MS: f32 = 200.0;

// ------------------------------------------------------------------ room

// Freeverb's tunings are in samples at 44.1 kHz; scale to the actual rate.
const COMB_TUNINGS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNINGS: [usize; 4] = [556, 441, 341, 225];
const FIXED_GAIN: f32 = 0.015;

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    pos: usize,
    store: f32,
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0, store: 0.0, feedback: 0.5, damp1: 0.5, damp2: 0.5 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let out = self.buf[self.pos];
        self.store = out * self.damp2 + self.store * self.damp1;
        self.buf[self.pos] = x + self.store * self.feedback;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

#[derive(Debug, Clone)]
struct FvAllpass {
    buf: Vec<f32>,
    pos: usize,
}

impl FvAllpass {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(1)], pos: 0 }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let buffered = self.buf[self.pos];
        let out = -x + buffered;
        self.buf[self.pos] = x + buffered * 0.5;
        self.pos = (self.pos + 1) % self.buf.len();
        out
    }
}

#[derive(Debug, Clone)]
struct Room {
    combs: Vec<Comb>,
    allpasses: Vec<FvAllpass>,
}

impl Room {
    fn new(sample_rate: u32) -> Self {
        let scale = sample_rate as f32 / 44_100.0;
        Self {
            combs: COMB_TUNINGS.iter().map(|&t| Comb::new((t as f32 * scale) as usize)).collect(),
            allpasses: ALLPASS_TUNINGS.iter().map(|&t| FvAllpass::new((t as f32 * scale) as usize)).collect(),
        }
    }
    fn configure(&mut self, room_size: f32, damping: f32) {
        let feedback = room_size.clamp(0.0, 1.0) * 0.28 + 0.7;
        let damp1 = damping.clamp(0.0, 1.0) * 0.4;
        for c in &mut self.combs {
            c.feedback = feedback;
            c.damp1 = damp1;
            c.damp2 = 1.0 - damp1;
        }
    }
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let input = x * FIXED_GAIN;
        let mut wet = 0.0;
        for c in &mut self.combs {
            wet += c.process(input);
        }
        for a in &mut self.allpasses {
            wet = a.process(wet);
        }
        wet
    }
}

// ------------------------------------------------------------------ plate

/// Plate tunings at Dattorro's 29761 Hz design rate.
const PLATE_RATE: f32 = 29_761.0;
const IN_DIFFUSERS: [(usize, f32); 4] = [(142, 0.75), (107, 0.75), (379, 0.625), (277, 0.625)];
/// Left tank: modulated allpass, delay, allpass, delay. Right likewise.
const TANK_L: [usize; 4] = [672, 4453, 1800, 3720];
const TANK_R: [usize; 4] = [908, 4217, 2656, 3163];
/// Output taps for the left channel: (which line, offset, sign). Lines are
/// 0 = right delay 1, 1 = right allpass 2, 2 = right delay 2, 3 = left delay 1,
/// 4 = left allpass 2, 5 = left delay 2.
const TAPS: [(usize, usize, f32); 7] = [
    (0, 266, 1.0),
    (0, 2974, 1.0),
    (1, 1913, -1.0),
    (2, 1996, 1.0),
    (3, 1990, -1.0),
    (4, 187, -1.0),
    (5, 1066, -1.0),
];
/// Peak modulation excursion of the tank allpasses, in design-rate samples.
const MOD_EXCURSION: f32 = 16.0;
const MOD_HZ: f32 = 1.0;

/// A plain delay line with fractional reads.
#[derive(Debug, Clone)]
struct Line {
    buf: Vec<f32>,
    pos: usize,
}

impl Line {
    fn new(len: usize) -> Self {
        Self { buf: vec![0.0; len.max(2)], pos: 0 }
    }
    #[inline]
    fn write(&mut self, x: f32) {
        self.buf[self.pos] = x;
        self.pos = (self.pos + 1) % self.buf.len();
    }
    /// Value written `d` samples ago (1 = the most recent write).
    #[inline]
    fn tap(&self, d: usize) -> f32 {
        let len = self.buf.len();
        self.buf[(self.pos + len - d.clamp(1, len)) % len]
    }
    /// Fractional tap, linear interpolation (excursions are small).
    #[inline]
    fn tap_frac(&self, d: f32) -> f32 {
        let i = d.floor();
        let f = d - i;
        let i = i as usize;
        self.tap(i) * (1.0 - f) + self.tap(i + 1) * f
    }
    /// The oldest value (a full-length delay).
    #[inline]
    fn out(&self) -> f32 {
        self.buf[self.pos]
    }
}

/// Schroeder allpass on a delay line, with a fractional, modulatable length.
#[derive(Debug, Clone)]
struct Allpass {
    line: Line,
    len: f32,
}

impl Allpass {
    fn new(len: usize, extra: usize) -> Self {
        Self { line: Line::new(len + extra + 2), len: len as f32 }
    }
    #[inline]
    fn process(&mut self, x: f32, g: f32, offset: f32) -> f32 {
        let d = self.line.tap_frac((self.len + offset).max(1.0));
        let v = x - g * d;
        self.line.write(v);
        d + g * v
    }
}

#[derive(Debug, Clone)]
struct Plate {
    bandwidth: f32,
    bw_state: f32,
    diffusers: Vec<(Allpass, f32)>,
    mod_ap: [Allpass; 2],
    delay1: [Line; 2],
    damp_state: [f32; 2],
    ap2: [Allpass; 2],
    delay2: [Line; 2],
    decay: f32,
    damping: f32,
    excursion: f32,
    phase: f32,
    phase_inc: f32,
    scale: f32,
}

impl Plate {
    fn new(sample_rate: u32) -> Self {
        let scale = sample_rate as f32 / PLATE_RATE;
        let s = |n: usize| (n as f32 * scale).round() as usize;
        let extra = (MOD_EXCURSION * scale).ceil() as usize + 2;
        Self {
            bandwidth: 0.9995,
            bw_state: 0.0,
            diffusers: IN_DIFFUSERS.iter().map(|&(n, g)| (Allpass::new(s(n), 0), g)).collect(),
            mod_ap: [Allpass::new(s(TANK_L[0]), extra), Allpass::new(s(TANK_R[0]), extra)],
            delay1: [Line::new(s(TANK_L[1])), Line::new(s(TANK_R[1]))],
            damp_state: [0.0; 2],
            ap2: [Allpass::new(s(TANK_L[2]), 0), Allpass::new(s(TANK_R[2]), 0)],
            delay2: [Line::new(s(TANK_L[3])), Line::new(s(TANK_R[3]))],
            decay: 0.5,
            damping: 0.0005,
            excursion: 0.0,
            phase: 0.0,
            phase_inc: MOD_HZ / sample_rate as f32,
            scale,
        }
    }

    fn configure(&mut self, room_size: f32, damping: f32, modulation: f32) {
        self.decay = 0.25 + 0.7 * room_size.clamp(0.0, 1.0);
        self.damping = 0.0005 + 0.7 * damping.clamp(0.0, 1.0);
        self.excursion = MOD_EXCURSION * self.scale * modulation.clamp(0.0, 1.0);
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        // Input bandwidth limit, then four diffusers.
        self.bw_state += self.bandwidth * (x - self.bw_state);
        let mut v = self.bw_state;
        for (ap, g) in &mut self.diffusers {
            v = ap.process(v, *g, 0.0);
        }

        // Tank: each half feeds the other, through a modulated allpass, a
        // delay, damping, decay, a second allpass and a second delay.
        self.phase = (self.phase + self.phase_inc).fract();
        let m = (std::f32::consts::TAU * self.phase).sin() * self.excursion;
        let dd2 = (self.decay + 0.15).clamp(0.25, 0.5);
        let feed = [self.delay2[1].out() * self.decay, self.delay2[0].out() * self.decay];
        for side in 0..2 {
            let offset = if side == 0 { m } else { -m };
            let a = self.mod_ap[side].process(v + feed[side], -0.70, offset);
            self.delay1[side].write(a);
            let d = self.delay1[side].out();
            self.damp_state[side] += (1.0 - self.damping) * (d - self.damp_state[side]);
            let b = self.ap2[side].process(self.damp_state[side] * self.decay, dd2, 0.0);
            self.delay2[side].write(b);
        }

        // Mono output from the left tap set.
        let s = self.scale;
        let lines: [&dyn Fn(usize) -> f32; 6] = [
            &|d| self.delay1[1].tap(d),
            &|d| self.ap2[1].line.tap(d),
            &|d| self.delay2[1].tap(d),
            &|d| self.delay1[0].tap(d),
            &|d| self.ap2[0].line.tap(d),
            &|d| self.delay2[0].tap(d),
        ];
        let mut y = 0.0;
        for &(line, off, sign) in &TAPS {
            y += sign * lines[line]((off as f32 * s).round() as usize);
        }
        y * 0.6
    }
}

// ------------------------------------------------------------------ reverb

#[derive(Debug, Clone)]
pub struct Reverb {
    sample_rate: u32,
    kind: ReverbKind,
    room: Room,
    plate: Plate,
    predelay: Line,
    predelay_samples: usize,
    lowcut: Option<Svf>,
    highcut: Option<Svf>,
    mix: Smoothed,
}

impl Reverb {
    pub fn new(sample_rate: u32, settings: ReverbSettings) -> Self {
        let mut r = Self {
            sample_rate,
            kind: settings.kind,
            room: Room::new(sample_rate),
            plate: Plate::new(sample_rate),
            predelay: Line::new((MAX_PREDELAY_MS * 0.001 * sample_rate as f32) as usize + 2),
            predelay_samples: 0,
            lowcut: None,
            highcut: None,
            mix: Smoothed::new(settings.mix, sample_rate, GLIDE_MS),
        };
        r.configure(settings);
        r
    }

    pub fn configure(&mut self, s: ReverbSettings) {
        self.kind = s.kind;
        self.room.configure(s.room_size, s.damping);
        self.plate.configure(s.room_size, s.damping, s.modulation);
        self.predelay_samples = (s.predelay_ms.clamp(0.0, MAX_PREDELAY_MS) * 0.001 * self.sample_rate as f32) as usize;
        self.mix.set(s.mix.clamp(0.0, 1.0));
        let sr = self.sample_rate;
        let set = |slot: &mut Option<Svf>, on: bool, shape: SvfShape, hz: f32| {
            if on {
                match slot {
                    Some(f) => f.set(shape, sr, hz, 0.7071, 0.0),
                    None => *slot = Some(Svf::new(shape, sr, hz, 0.7071, 0.0)),
                }
            } else {
                *slot = None;
            }
        };
        set(&mut self.lowcut, s.lowcut_hz > 0.0, SvfShape::HighPass, s.lowcut_hz);
        set(&mut self.highcut, s.highcut_hz < 19_500.0, SvfShape::LowPass, s.highcut_hz);
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.predelay.write(x);
        let input = if self.predelay_samples == 0 { x } else { self.predelay.tap(self.predelay_samples + 1) };
        let mut wet = match self.kind {
            ReverbKind::Room => self.room.process(input),
            ReverbKind::Plate => self.plate.process(input),
        };
        if let Some(f) = &mut self.lowcut {
            wet = f.process(wet);
        }
        if let Some(f) = &mut self.highcut {
            wet = f.process(wet);
        }
        let mix = self.mix.next();
        x * (1.0 - mix) + wet * mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reverb(f: impl FnOnce(&mut ReverbSettings)) -> Reverb {
        let mut s = ReverbSettings { mix: 1.0, predelay_ms: 0.0, lowcut_hz: 0.0, highcut_hz: 20_000.0, ..Default::default() };
        f(&mut s);
        Reverb::new(48_000, s)
    }

    /// Energy in successive 100 ms windows after an impulse.
    fn decay_profile(r: &mut Reverb, seconds: f32) -> Vec<f64> {
        r.process(1.0);
        let n = (48_000.0 * seconds) as usize;
        let mut windows = vec![0.0f64; n / 4800 + 1];
        for i in 0..n {
            let y = r.process(0.0);
            assert!(y.is_finite());
            windows[i / 4800] += (y as f64).powi(2);
        }
        windows
    }

    #[test]
    fn both_types_ring_out_and_decay() {
        for kind in ReverbKind::ALL {
            let w = decay_profile(&mut reverb(|s| s.kind = kind), 3.0);
            assert!(w[1] > 0.0, "{kind:?}: no tail");
            assert!(w[25] < w[2] * 0.1, "{kind:?}: tail should decay: {} vs {}", w[25], w[2]);
        }
    }

    #[test]
    fn bigger_size_rings_longer() {
        for kind in ReverbKind::ALL {
            let small = decay_profile(&mut reverb(|s| { s.kind = kind; s.room_size = 0.2; }), 2.0);
            let big = decay_profile(&mut reverb(|s| { s.kind = kind; s.room_size = 0.9; }), 2.0);
            assert!(big[12] > small[12] * 2.0, "{kind:?}: size barely matters: {} vs {}", big[12], small[12]);
        }
    }

    #[test]
    fn predelay_holds_back_the_tail() {
        let mut r = reverb(|s| s.predelay_ms = 50.0);
        r.process(1.0);
        let early: f32 = (0..2000).map(|_| r.process(0.0).abs()).sum(); // first ~42 ms
        assert!(early < 1e-6, "tail started before the pre-delay: {early}");
    }

    #[test]
    fn plate_stays_stable_at_maximum_size() {
        let w = decay_profile(&mut reverb(|s| { s.room_size = 1.0; s.damping = 0.0; s.modulation = 1.0; }), 10.0);
        let last = *w.last().unwrap();
        let peak = w.iter().cloned().fold(0.0, f64::max);
        assert!(last < peak, "maximum size must still decay");
    }

    #[test]
    fn dry_mix_is_transparent() {
        let mut r = reverb(|s| s.mix = 0.0);
        for i in 0..1000 {
            let x = (i as f32 * 0.1).sin();
            assert!((r.process(x) - x).abs() < 1e-6);
        }
    }
}
