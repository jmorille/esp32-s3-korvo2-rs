// SPDX-License-Identifier: MIT OR Apache-2.0
//! Audio path check: logs the RMS level of MIC1, MIC2 and the AEC reference
//! channel every 500 ms while cycling through three phases:
//!
//! 1. `silence`  – nothing played: REF should be near the noise floor.
//! 2. `tone`     – 1 kHz tone on the speaker: REF must rise sharply (it is the
//!    DAC output looped into ES7210 MIC3), the mics pick up the acoustic echo.
//! 3. `loopback` – MIC1 played back on the speaker (attenuated, low volume).
//!
//! It also logs the battery voltage at startup and every button press / release.
//!
//! Set `KORVO2_SAMPLE_RATE=48000` (or 32000, 24000, 22050) at build time to run
//! the shared I2S clock at another rate; the microphones are then decimated to
//! 16 kHz when the rate is 32 or 48 kHz.
//!
//! `cargo run --release --example loopback`

#[path = "common/mod.rs"]
mod common;

use std::time::{Duration, Instant};

use common::{dbfs, rms, Synth};
use esp32_s3_korvo2::{Config, Korvo2, Led, Mic};

const FRAMES: usize = 256; // 16 ms at 16 kHz
const PHASE: Duration = Duration::from_secs(4);

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let rate: u32 = option_env!("KORVO2_SAMPLE_RATE")
        .and_then(|r| r.parse().ok())
        .unwrap_or(16_000);
    let board = Korvo2::take_with(Config {
        sample_rate: rate,
        ..Default::default()
    })?;
    let (mut mic, mut speaker, mut buttons, mut battery, leds) = (
        board.mic,
        board.speaker,
        board.buttons,
        board.battery,
        board.leds,
    );
    log::info!(
        "battery: {} mV (GPIO6 {} mV, ~{} %)",
        battery.read_millivolts()?,
        battery.read_pin_millivolts()?,
        battery.percent()?
    );
    log::info!("buttons ladder idle: {} mV", buttons.read_millivolts()?);
    speaker.set_volume(50)?;
    let (mic_rate, spk_rate) = (mic.sample_rate(), speaker.sample_rate());
    log::info!("I2S {rate} Hz: mic frames at {mic_rate} Hz, speaker at {spk_rate} Hz");
    let ratio = (spk_rate as usize).div_ceil(mic_rate as usize).max(1);

    let mut input = vec![0i16; FRAMES * Mic::CHANNELS];
    // Same duration as the input block at the speaker rate.
    let mut output = vec![0i16; FRAMES * spk_rate as usize / mic_rate as usize];
    let mut synth = Synth::new(spk_rate);
    let phases = ["silence", "tone", "loopback"];
    let start = Instant::now();
    let mut acc = [0f32; 3];
    let mut n = 0u32;
    let mut last_log = Instant::now();

    loop {
        let phase = phases[(start.elapsed().as_secs() / PHASE.as_secs()) as usize % phases.len()];
        mic.read(&mut input)?;
        for (c, a) in acc.iter_mut().enumerate() {
            let r = rms(&input, Mic::CHANNELS, c);
            *a += r * r;
        }
        n += 1;

        match phase {
            "tone" => synth.tone(&mut output, 1000.0, 0.3),
            "loopback" => {
                // Crude sample-and-hold upsampling when the speaker runs faster.
                for (i, o) in output.iter_mut().enumerate() {
                    *o = input[(i / ratio).min(FRAMES - 1) * Mic::CHANNELS] / 2;
                }
            }
            _ => output.fill(0),
        }
        speaker.write(&output)?;
        if let Some(ev) = buttons.poll()? {
            log::info!("button {ev:?}");
            leds.toggle(Led::Blue)?;
        }

        if last_log.elapsed() >= Duration::from_millis(500) {
            let r: Vec<f32> = acc.iter().map(|a| (a / n as f32).sqrt()).collect();
            log::info!(
                "[{phase:8}] RMS mic1={:6.0} ({:5.1} dBFS)  mic2={:6.0} ({:5.1} dBFS)  ref={:6.0} ({:5.1} dBFS)",
                r[0],
                dbfs(r[0]),
                r[1],
                dbfs(r[1]),
                r[2],
                dbfs(r[2])
            );
            acc = [0.0; 3];
            n = 0;
            last_log = Instant::now();
        }
    }
}
