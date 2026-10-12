//! Chorus: one to three modulated delay voices mixed with the dry signal.
//!
//! References (docs/research/effects-references.md, section 8): Dattorro,
//! "Effect Design Part 2" (JAES 1997) for the modulated delay line and
//! feedback; cubic Hermite interpolation (clean under modulation); and the
//! public behavior of BBD choruses (a darker wet path, rate and depth only).

use std::f32::consts::TAU;

use crate::dsp::smooth::{Smoothed, GLIDE_MS};
use crate::dsp::svf::OnePoleLp;
use crate::types::Sample;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChorusSettings {
    /// How far each voice's delay swings, ms (peak).
    pub depth_ms: f32,
    pub rate_hz: f32,
    pub mix: f32,
    /// Center delay of the voices, ms.
    pub base_ms: f32,
    /// 1 to 3. More voices with spread LFO phases sound thicker.
    pub voices: u32,
    /// -0.7 to 0.7. Toward the ends it turns into a flanger.
    pub feedback: f32,
    /// Low-pass on the wet signal, Hz (20 kHz and up = off).
    pub highcut_hz: f32,
}

impl Default for ChorusSettings {
    fn default() -> Self {
        Self { depth_ms: 2.0, rate_hz: 0.6, mix: 0.35, base_ms: 12.0, voices: 2, feedback: 0.0, highcut_hz: 10_000.0 }
    }
}

#[derive(Debug, Clone)]
pub struct Chorus {
    sample_rate: f32,
    s: ChorusSettings,
    buf: Vec<Sample>,
    pos: usize,
    phase: f32,
    depth: Smoothed,
    base: Smoothed,
    mix: Smoothed,
    tone: OnePoleLp,
    tone_on: bool,
    last_wet: f32,
}

/// Longest base plus depth, plus room for interpolation.
const MAX_MS: f32 = 60.0;

impl Chorus {
    pub fn new(sample_rate: u32, settings: ChorusSettings) -> Self {
        let sr = sample_rate as f32;
        let mut c = Self {
            sample_rate: sr,
            s: settings,
            buf: vec![0.0; (MAX_MS * 0.001 * sr) as usize + 8],
            pos: 0,
            phase: 0.0,
            depth: Smoothed::new(settings.depth_ms, sample_rate, GLIDE_MS),
            base: Smoothed::new(settings.base_ms, sample_rate, GLIDE_MS),
            mix: Smoothed::new(settings.mix, sample_rate, GLIDE_MS),
            tone: OnePoleLp::new(sample_rate, settings.highcut_hz),
            tone_on: false,
            last_wet: 0.0,
        };
        c.configure(settings);
        c
    }

    pub fn configure(&mut self, settings: ChorusSettings) {
        let mut s = settings;
        s.base_ms = s.base_ms.clamp(1.0, 30.0);
        s.depth_ms = s.depth_ms.clamp(0.0, (s.base_ms - 0.5).min(15.0));
        s.rate_hz = s.rate_hz.clamp(0.02, 10.0);
        s.mix = s.mix.clamp(0.0, 1.0);
        s.voices = s.voices.clamp(1, 3);
        s.feedback = s.feedback.clamp(-0.7, 0.7);
        self.depth.set(s.depth_ms);
        self.base.set(s.base_ms);
        self.mix.set(s.mix);
        self.tone_on = s.highcut_hz < 19_500.0;
        if self.tone_on {
            self.tone.set(self.sample_rate as u32, s.highcut_hz);
        }
        self.s = s;
    }

    /// Cubic Hermite read `delay` samples behind the write head.
    #[inline]
    fn read(&self, delay: f32) -> f32 {
        let len = self.buf.len();
        let pos = self.pos as f32 - delay + len as f32;
        let i = pos.floor();
        let t = pos - i;
        let i = i as usize;
        let at = |k: usize| self.buf[(i + len + k - 1) % len];
        let (xm1, x0, x1, x2) = (at(0), at(1), at(2), at(3));
        let c1 = 0.5 * (x1 - xm1);
        let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
        let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
        ((c3 * t + c2) * t + c1) * t + x0
    }

    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.buf[self.pos] = x + self.s.feedback * self.last_wet;

        let depth = self.depth.next();
        let base = self.base.next();
        let n = self.s.voices;
        let mut wet = 0.0;
        for v in 0..n {
            // Spread the voices evenly around the LFO cycle.
            let lfo = (TAU * (self.phase + v as f32 / n as f32)).sin();
            let delay = ((base + depth * lfo) * 0.001 * self.sample_rate).max(2.0);
            wet += self.read(delay);
        }
        wet /= n as f32;
        self.last_wet = wet;
        if self.tone_on {
            wet = self.tone.process(wet);
        }

        self.pos = (self.pos + 1) % self.buf.len();
        self.phase = (self.phase + self.s.rate_hz / self.sample_rate).fract();
        let mix = self.mix.next();
        x * (1.0 - mix) + wet * mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chorus(f: impl FnOnce(&mut ChorusSettings)) -> Chorus {
        let mut s = ChorusSettings::default();
        f(&mut s);
        Chorus::new(48_000, s)
    }

    #[test]
    fn dry_when_mix_zero_and_finite_when_wet() {
        let mut dry = chorus(|s| s.mix = 0.0);
        assert!((dry.process(0.5) - 0.5).abs() < 1e-6);
        let mut wet = chorus(|s| s.mix = 0.5);
        for i in 0..2000 {
            assert!(wet.process((i as f32 * 0.01).sin()).is_finite());
        }
    }

    /// The wet signal's pitch wobbles: a steady tone comes out with its
    /// zero crossings moving back and forth.
    #[test]
    fn modulates_the_wet_signal() {
        let mut c = chorus(|s| {
            s.mix = 1.0;
            s.voices = 1;
            s.depth_ms = 5.0;
            s.rate_hz = 2.0;
            s.highcut_hz = 20_000.0;
        });
        let out: Vec<f32> = (0..48_000).map(|i| c.process((TAU * 500.0 * i as f32 / 48_000.0).sin())).collect();
        // Period lengths between rising zero crossings must vary.
        let mut periods = Vec::new();
        let mut last = None;
        for i in 2000..out.len() {
            if out[i - 1] < 0.0 && out[i] >= 0.0 {
                if let Some(l) = last {
                    periods.push((i - l) as f32);
                }
                last = Some(i);
            }
        }
        let (lo, hi) = periods.iter().fold((f32::MAX, 0.0f32), |(a, b), &p| (a.min(p), b.max(p)));
        assert!(hi - lo >= 2.0, "pitch should wobble: periods {lo}..{hi}");
    }

    #[test]
    fn feedback_stays_bounded() {
        let mut c = chorus(|s| {
            s.feedback = 0.7;
            s.mix = 1.0;
        });
        let mut peak = 0.0f32;
        for i in 0..96_000 {
            let y = c.process((TAU * 300.0 * i as f32 / 48_000.0).sin());
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 4.0, "feedback runaway: {peak}");
    }

    #[test]
    fn depth_is_clamped_below_base() {
        let c = chorus(|s| {
            s.base_ms = 5.0;
            s.depth_ms = 15.0;
        });
        assert!(c.s.depth_ms < 5.0, "depth must stay below the base delay");
    }
}
