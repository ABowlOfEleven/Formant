//! RNNoise denoiser (via `nnnoiseless`), adapted to arbitrary block sizes.

use std::collections::VecDeque;

use nnnoiseless::DenoiseState;

use crate::types::Sample;

/// RNNoise frame size: 480 samples = 10 ms at 48 kHz.
const FRAME: usize = DenoiseState::FRAME_SIZE;

/// RNNoise expects i16-range float PCM, not normalized `[-1, 1]`.
const SCALE: f32 = 32768.0;

/// CPU noise suppression. RNNoise only consumes fixed 480-sample frames, so this
/// buffers the incoming stream into frames, runs the model, and emits samples
/// with exactly one frame (~10 ms) of latency — primed with silence so the
/// output length always matches the input with no underflow.
///
/// Bonus: each frame yields a voice-activity probability, which the gate can use
/// for "VAD by default" muting without a separate detector.
pub struct Denoise {
    state: Box<DenoiseState<'static>>,
    in_buf: VecDeque<Sample>,
    out_buf: VecDeque<Sample>,
    frame_in: [f32; FRAME],
    frame_out: [f32; FRAME],
    vad: f32,
}

impl Denoise {
    pub fn new() -> Self {
        let mut out_buf = VecDeque::with_capacity(FRAME * 4);
        // Prime one frame of latency so output never starves input.
        out_buf.extend(std::iter::repeat(0.0).take(FRAME));
        Self {
            state: DenoiseState::new(),
            in_buf: VecDeque::with_capacity(FRAME * 4),
            out_buf,
            frame_in: [0.0; FRAME],
            frame_out: [0.0; FRAME],
            vad: 0.0,
        }
    }

    /// Most recent voice-activity probability in `[0, 1]`.
    pub fn vad(&self) -> f32 {
        self.vad
    }

    /// Process one sample. Returns a denoised sample delayed by ~10 ms.
    #[inline]
    pub fn process(&mut self, x: Sample) -> Sample {
        self.in_buf.push_back(x);

        if self.in_buf.len() >= FRAME {
            for slot in self.frame_in.iter_mut() {
                *slot = self.in_buf.pop_front().unwrap() * SCALE;
            }
            self.vad = self.state.process_frame(&mut self.frame_out, &self.frame_in);
            for &s in self.frame_out.iter() {
                self.out_buf.push_back(s / SCALE);
            }
        }

        self.out_buf.pop_front().unwrap_or(0.0)
    }
}

impl Default for Denoise {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduces_steady_noise_and_reports_vad() {
        let mut denoise = Denoise::new();

        // Deterministic pseudo-noise (LCG) so the test is reproducible.
        let mut seed = 0x9E3779B9u32;
        let mut noise = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((seed >> 8) as f32 / 16_777_216.0 - 0.5) * 0.6
        };

        let n = 48_000; // 1 second
        let (mut in_sq, mut out_sq, mut count) = (0.0f64, 0.0f64, 0u64);
        let mut last_vad = 0.0;
        for i in 0..n {
            let x = noise();
            let y = denoise.process(x);
            last_vad = denoise.vad();
            // Measure the steady-state second half (past warmup + latency).
            if i > n / 2 {
                in_sq += (x as f64).powi(2);
                out_sq += (y as f64).powi(2);
                count += 1;
            }
        }

        let in_rms = (in_sq / count as f64).sqrt();
        let out_rms = (out_sq / count as f64).sqrt();
        assert!(out_rms.is_finite());
        assert!(
            out_rms < in_rms,
            "RNNoise should attenuate steady noise: in {in_rms:.4} out {out_rms:.4}"
        );
        assert!((0.0..=1.0).contains(&last_vad), "VAD out of range: {last_vad}");
    }
}
