// SPDX-License-Identifier: MIT OR Apache-2.0
//! Helpers shared by the examples (signal levels and test signal synthesis).
#![allow(dead_code)]

/// RMS of one channel of an interleaved buffer.
pub fn rms(buf: &[i16], channels: usize, channel: usize) -> f32 {
    let mut sum = 0f64;
    let mut n = 0usize;
    for s in buf.iter().skip(channel).step_by(channels) {
        sum += f64::from(*s) * f64::from(*s);
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        (sum / n as f64).sqrt() as f32
    }
}

/// RMS expressed in dBFS (full scale = 32768).
pub fn dbfs(rms: f32) -> f32 {
    20.0 * (rms.max(1.0) / 32768.0).log10()
}

/// Test signal generator. Signals are rendered once into a table and then looped,
/// so that the player thread stays cheap (libm `sinf`/`powf` per sample would eat
/// a large share of a core and starve lower priority tasks).
pub struct Synth {
    rate: f32,
    t: u64,
    babble: Vec<i16>,
    pos: usize,
}

impl Synth {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate as f32,
            t: 0,
            babble: Vec::new(),
            pos: 0,
        }
    }

    /// Pure sine tone.
    pub fn tone(&mut self, out: &mut [i16], freq: f32, amplitude: f32) {
        for s in out.iter_mut() {
            let phase = (freq * self.t as f32 / self.rate).fract();
            let x = (2.0 * core::f32::consts::PI * phase).sin();
            *s = (x * amplitude * 32767.0) as i16;
            self.t += 1;
        }
    }

    /// Render the babble table now (takes a few seconds on the ESP32-S3), so
    /// that the first [`Synth::babble`] call does not stall the player.
    pub fn prepare_babble(&mut self, amplitude: f32) {
        self.babble(&mut [], amplitude);
    }

    /// Speech-like "babble": a 140 Hz harmonic series whose pitch wobbles and
    /// whose amplitude is modulated at ~4 syllables per second. Good enough to
    /// trigger a VAD, so it is a meaningful echo cancellation test signal.
    pub fn babble(&mut self, out: &mut [i16], amplitude: f32) {
        if self.babble.is_empty() {
            // 2 s loop (8 syllables).
            let mut table = vec![0i16; 2 * self.rate as usize];
            self.render_babble(&mut table, amplitude);
            self.babble = table;
        }
        for s in out.iter_mut() {
            *s = self.babble[self.pos];
            self.pos = (self.pos + 1) % self.babble.len();
        }
    }

    fn render_babble(&mut self, out: &mut [i16], amplitude: f32) {
        use core::f32::consts::PI;
        self.t = 0;
        for s in out.iter_mut() {
            let t = self.t as f32 / self.rate;
            let f0 = 140.0 + 25.0 * (2.0 * PI * 0.7 * t).sin();
            let mut x = 0.0;
            for h in 1..=8 {
                let formant = 1.0 / (1.0 + ((h as f32 * f0 - 700.0) / 400.0).powi(2));
                x += formant * (2.0 * PI * h as f32 * f0 * t).sin();
            }
            let env = (0.5 - 0.5 * (2.0 * PI * 4.0 * t).cos()).powf(1.5);
            *s = (0.3 * x * env * amplitude * 32767.0).clamp(-32767.0, 32767.0) as i16;
            self.t += 1;
        }
    }
}

/// Spawn a std thread with a given FreeRTOS priority and core affinity.
///
/// ESP-SR is CPU hungry: run the AFE fetch / detection loop on core 1 at a
/// higher priority than the main task (priority 1), or the AFE ring buffer
/// overflows ("Ringbuffer of AFE(FEED) is full").
pub fn spawn_pinned<F>(
    name: &'static core::ffi::CStr,
    priority: u8,
    core: Option<esp_idf_svc::hal::cpu::Core>,
    stack_size: usize,
    f: F,
) -> anyhow::Result<std::thread::JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
    ThreadSpawnConfiguration {
        name: Some(name),
        stack_size,
        priority,
        pin_to_core: core,
        ..Default::default()
    }
    .set()?;
    let h = std::thread::Builder::new()
        .stack_size(stack_size)
        .spawn(f)?;
    ThreadSpawnConfiguration::default().set()?;
    Ok(h)
}
