//! UI-side spectrum analysis: FFT the recent output samples into log-spaced
//! bands for the Mixer display. Runs on the UI thread, not the audio thread.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

const FRAME: usize = 1024;
const BINS: usize = FRAME / 2 + 1;
/// Number of display bars.
pub const BANDS: usize = 56;

pub struct Spectrum {
    r2c: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<Complex<f32>>,
    window: Vec<f32>,
    frame: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    /// Smoothed magnitude per band, 0..1 for drawing.
    pub bands: [f32; BANDS],
}

impl Spectrum {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(FRAME);
        let window: Vec<f32> = (0..FRAME)
            .map(|k| 0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / FRAME as f32).cos())
            .collect();
        Self {
            scratch: vec![Complex::new(0.0, 0.0); r2c.get_scratch_len()],
            r2c,
            window,
            frame: vec![0.0; FRAME],
            spectrum: vec![Complex::new(0.0, 0.0); BINS],
            bands: [0.0; BANDS],
        }
    }

    /// Update the bands from the most recent samples (uses the last FRAME).
    pub fn update(&mut self, samples: &[f32]) {
        if samples.len() < FRAME {
            return;
        }
        let start = samples.len() - FRAME;
        for k in 0..FRAME {
            self.frame[k] = samples[start + k] * self.window[k];
        }
        if self.r2c.process_with_scratch(&mut self.frame, &mut self.spectrum, &mut self.scratch).is_err() {
            return;
        }

        // Aggregate bins into log-spaced bands, take the peak magnitude in each.
        let min_bin = 1usize;
        let max_bin = BINS - 1;
        for b in 0..BANDS {
            let lo = log_bin(b, BANDS, min_bin, max_bin);
            let hi = log_bin(b + 1, BANDS, min_bin, max_bin).max(lo + 1);
            let mut mag = 0.0f32;
            for k in lo..hi.min(BINS) {
                let m = self.spectrum[k].norm();
                if m > mag {
                    mag = m;
                }
            }
            // To dB, then map a useful range to 0..1.
            let db = 20.0 * (mag / FRAME as f32 + 1e-9).log10();
            let target = ((db + 78.0) / 78.0).clamp(0.0, 1.0);
            // Fast attack, slow release so bars are readable.
            let prev = self.bands[b];
            self.bands[b] = if target > prev { target } else { prev * 0.82 + target * 0.18 };
        }
    }
}

fn log_bin(b: usize, bands: usize, min_bin: usize, max_bin: usize) -> usize {
    let t = b as f32 / bands as f32;
    let lo = (min_bin as f32).ln();
    let hi = (max_bin as f32).ln();
    (lo + (hi - lo) * t).exp().round() as usize
}
