//! Feedback delay with tape character.
//!
//! References (docs/research/effects-references.md, section 8): Dattorro,
//! "Effect Design Part 2" (JAES 1997) on modulated delay lines; Niemitalo's
//! polynomial interpolators (cubic Hermite, which stays clean under
//! modulation where allpass interpolation does not); and the public behavior
//! of tape echoes (filtered, saturating feedback; wow and flutter; a pitch
//! slide when the time changes).

use std::f32::consts::TAU;

use crate::dsp::smooth::{Smoothed, GLIDE_MS};
use crate::dsp::svf::{Svf, SvfShape};
use crate::types::Sample;

/// Longest delay offered, and the buffer headroom for modulation and
/// interpolation on top of it.
pub const MAX_TIME_MS: f32 = 2000.0;
const HEADROOM_MS: f32 = 60.0;
/// How long a time change takes to settle. Long enough to slide like tape
/// rather than jump; the slide is part of the sound.
const TIME_GLIDE_MS: f32 = 120.0;
/// Wow and flutter rates, and their depth at full setting (fraction of the
/// delay time).
const WOW_HZ: f32 = 0.55;
const FLUTTER_HZ: f32 = 7.3;
const WOW_DEPTH: f32 = 0.004;
const FLUTTER_DEPTH: f32 = 0.0008;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DelaySettings {
    pub time_ms: f32,
    /// 0 to 1.05. Above about 0.95 the repeats only stay bounded because the
    /// feedback saturates.
    pub feedback: f32,
    pub mix: f32,
    /// High-pass in the feedback path, Hz (0 = off).
    pub lowcut_hz: f32,
    /// Low-pass in the feedback path, Hz (20 kHz and up = off).
    pub highcut_hz: f32,
    /// Saturation in the feedback path, 0 to 1.
    pub drive: f32,
    pub wow: f32,
    pub flutter: f32,
}

impl Default for DelaySettings {
    fn default() -> Self {
        Self {
            time_ms: 280.0,
            feedback: 0.3,
            mix: 0.2,
            lowcut_hz: 150.0,
            highcut_hz: 5000.0,
            drive: 0.2,
            wow: 0.1,
            flutter: 0.05,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Delay {
    sample_rate: f32,
    s: DelaySettings,
    buf: Vec<Sample>,
    pos: usize,
    /// Delay in samples, gliding.
    time: Smoothed,
    mix: Smoothed,
    lowcut: Option<Svf>,
    highcut: Option<Svf>,
    wow_phase: f32,
    flutter_phase: f32,
}

impl Delay {
    pub fn new(sample_rate: u32, settings: DelaySettings) -> Self {
        let sr = sample_rate as f32;
        let capacity = ((MAX_TIME_MS + HEADROOM_MS) * 0.001 * sr) as usize + 8;
        let mut d = Self {
            sample_rate: sr,
            s: settings,
            buf: vec![0.0; capacity],
            pos: 0,
            time: Smoothed::new(Self::samples(sr, settings.time_ms), sample_rate, TIME_GLIDE_MS),
            mix: Smoothed::new(settings.mix, sample_rate, GLIDE_MS),
            lowcut: None,
            highcut: None,
            wow_phase: 0.0,
            flutter_phase: 0.0,
        };
        d.configure(settings);
        d
    }

    fn samples(sr: f32, ms: f32) -> f32 {
        (ms.clamp(1.0, MAX_TIME_MS) * 0.001 * sr).max(2.0)
    }

    pub fn configure(&mut self, settings: DelaySettings) {
        let mut s = settings;
        s.feedback = s.feedback.clamp(0.0, 1.05);
        s.mix = s.mix.clamp(0.0, 1.0);
        s.drive = s.drive.clamp(0.0, 1.0);
        s.wow = s.wow.clamp(0.0, 1.0);
        s.flutter = s.flutter.clamp(0.0, 1.0);
        self.time.set(Self::samples(self.sample_rate, s.time_ms));
        self.mix.set(s.mix);
        let sr = self.sample_rate as u32;
        let set = |slot: &mut Option<Svf>, on: bool, shape: SvfShape, hz: f32| {
            if on {
                match slot {
                    Some(f) => f.set(shape, sr, hz, crate::dsp::svf::BUTTERWORTH_Q, 0.0),
                    None => *slot = Some(Svf::new(shape, sr, hz, crate::dsp::svf::BUTTERWORTH_Q, 0.0)),
                }
            } else {
                *slot = None;
            }
        };
        set(&mut self.lowcut, s.lowcut_hz > 0.0, SvfShape::HighPass, s.lowcut_hz);
        set(&mut self.highcut, s.highcut_hz < 19_500.0, SvfShape::LowPass, s.highcut_hz);
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
        let base = self.time.next();
        // Wow is slow and sine-like; flutter is faster and smaller.
        self.wow_phase = (self.wow_phase + WOW_HZ / self.sample_rate).fract();
        self.flutter_phase = (self.flutter_phase + FLUTTER_HZ / self.sample_rate).fract();
        let wobble = self.s.wow * WOW_DEPTH * (TAU * self.wow_phase).sin()
            + self.s.flutter * FLUTTER_DEPTH * (TAU * self.flutter_phase).sin();
        let delay = (base * (1.0 + wobble)).max(2.0);
        let delayed = self.read(delay);

        // Feedback path: filter, then saturate. The saturation is what keeps
        // feedback near or above 1.0 bounded instead of exploding.
        let mut fb = delayed;
        if let Some(f) = &mut self.lowcut {
            fb = f.process(fb);
        }
        if let Some(f) = &mut self.highcut {
            fb = f.process(fb);
        }
        fb *= self.s.feedback;
        let k = 0.5 + 3.5 * self.s.drive;
        if self.s.drive > 0.0 || self.s.feedback > 0.95 {
            fb = (fb * k).tanh() / k;
        }

        self.buf[self.pos] = x + fb;
        self.pos = (self.pos + 1) % self.buf.len();
        let mix = self.mix.next();
        x * (1.0 - mix) + delayed * mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(time_ms: f32, feedback: f32) -> Delay {
        Delay::new(
            48_000,
            DelaySettings {
                time_ms,
                feedback,
                mix: 1.0,
                lowcut_hz: 0.0,
                highcut_hz: 20_000.0,
                drive: 0.0,
                wow: 0.0,
                flutter: 0.0,
            },
        )
    }

    #[test]
    fn echoes_after_the_delay_time() {
        let sr = 48_000;
        let mut d = clean(10.0, 0.0);
        let delay = (sr as f32 * 0.01) as usize;
        let first = d.process(1.0);
        assert!(first.abs() < 1e-6, "wet output is silent until the echo");
        for _ in 1..delay {
            d.process(0.0);
        }
        let echo = d.process(0.0);
        assert!((echo - 1.0).abs() < 1e-3, "impulse should reappear after the delay: {echo}");
    }

    /// Changing the time slides rather than jumps: the read position never
    /// skips, so a tone through the delay has no discontinuity.
    #[test]
    fn changing_time_glides_without_a_click() {
        let mut d = clean(200.0, 0.0);
        let mut prev = 0.0f32;
        let mut worst = 0.0f32;
        for i in 0..96_000 {
            if i == 48_000 {
                let mut s = d.s;
                s.time_ms = 350.0;
                d.configure(s);
            }
            let y = d.process(0.5 * (TAU * 440.0 * i as f32 / 48_000.0).sin());
            if i > 12_000 {
                worst = worst.max((y - prev).abs());
            }
            prev = y;
        }
        // The pitch slide changes the tone's speed a little; a jump would be
        // a step of up to the full 1.0 swing.
        assert!(worst < 0.1, "largest step {worst}");
    }

    /// Feedback above 1 with saturation stays bounded.
    #[test]
    fn runaway_feedback_stays_bounded() {
        let mut d = Delay::new(48_000, DelaySettings { time_ms: 50.0, feedback: 1.05, mix: 1.0, ..Default::default() });
        d.process(1.0);
        let mut peak = 0.0f32;
        for _ in 0..480_000 {
            let y = d.process(0.0);
            assert!(y.is_finite());
            peak = peak.max(y.abs());
        }
        assert!(peak < 2.0, "self-oscillation should stay bounded: {peak}");
    }

    /// The high-cut in the feedback path darkens each repeat.
    #[test]
    fn repeats_get_darker() {
        let energy_at = |hz: f32| {
            let mut d = Delay::new(
                48_000,
                DelaySettings { time_ms: 20.0, feedback: 0.7, mix: 1.0, highcut_hz: 2000.0, lowcut_hz: 0.0, drive: 0.0, wow: 0.0, flutter: 0.0 },
            );
            let mut e = 0.0f64;
            for i in 0..48_000 {
                let x = if i < 960 { (TAU * hz * i as f32 / 48_000.0).sin() } else { 0.0 };
                let y = d.process(x);
                if i > 4800 {
                    e += (y as f64).powi(2);
                }
            }
            e
        };
        assert!(energy_at(8000.0) < energy_at(500.0) * 0.1, "highs should die away faster in the repeats");
    }
}
