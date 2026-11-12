// SPDX-License-Identifier: MIT OR Apache-2.0
//! Small fixed-ratio decimator used to deliver 16 kHz microphone frames when the
//! shared I2S clock runs at 32 or 48 kHz.

/// Interleaved multi-channel FIR decimator by an integer factor.
pub(crate) struct Decimator {
    factor: usize,
    channels: usize,
    taps: Vec<f32>,
    /// Per channel history, stored twice (`2 * taps.len()`) so that the last
    /// `taps.len()` samples are always contiguous.
    hist: Vec<Vec<f32>>,
    pos: usize,
    phase: usize,
}

impl Decimator {
    /// Windowed-sinc low-pass (Hamming) with cut-off at 0.9 × the output Nyquist.
    pub(crate) fn new(factor: usize, channels: usize) -> Self {
        let n = 16 * factor + 1;
        let fc = 0.45 / factor as f32; // normalised to the input rate
        let mid = (n / 2) as f32;
        let mut taps: Vec<f32> = (0..n)
            .map(|i| {
                let x = i as f32 - mid;
                let sinc = if x == 0.0 {
                    2.0 * fc
                } else {
                    (2.0 * core::f32::consts::PI * fc * x).sin() / (core::f32::consts::PI * x)
                };
                let w =
                    0.54 - 0.46 * (2.0 * core::f32::consts::PI * i as f32 / (n - 1) as f32).cos();
                sinc * w
            })
            .collect();
        let sum: f32 = taps.iter().sum();
        taps.iter_mut().for_each(|t| *t /= sum);
        Self {
            factor,
            channels,
            hist: vec![vec![0.0; 2 * n]; channels],
            taps,
            pos: 0,
            phase: 0,
        }
    }

    /// Decimate `input` (interleaved, `channels` per frame) into `output`.
    /// `input.len()` must be `factor * output.len()`.
    pub(crate) fn process(&mut self, input: &[i16], output: &mut [i16]) {
        let n = self.taps.len();
        let mut out = output.chunks_exact_mut(self.channels);
        for frame in input.chunks_exact(self.channels) {
            for (c, s) in frame.iter().enumerate() {
                let v = f32::from(*s);
                self.hist[c][self.pos] = v;
                self.hist[c][self.pos + n] = v;
            }
            self.pos = (self.pos + 1) % n;
            self.phase += 1;
            if self.phase == self.factor {
                self.phase = 0;
                let Some(o) = out.next() else { return };
                for (c, h) in self.hist.iter().enumerate() {
                    // Oldest sample at `pos`, newest at `pos + n - 1`.
                    let window = &h[self.pos..self.pos + n];
                    let acc: f32 = window.iter().zip(&self.taps).map(|(x, t)| x * t).sum();
                    o[c] = acc.clamp(-32768.0, 32767.0) as i16;
                }
            }
        }
    }
}
