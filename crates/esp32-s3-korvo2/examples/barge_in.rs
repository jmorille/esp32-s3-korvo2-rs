// SPDX-License-Identifier: MIT OR Apache-2.0
//! Measures how fast the speaker goes silent when playback is interrupted
//! (barge-in), with and without [`SpeakerControl::clear`].
//!
//! A player thread streams a 1 kHz tone in 20 ms writes. Every 1.5 s the main
//! thread stops it, alternately calling `clear()` or just stopping the writes,
//! and measures on the AEC reference channel (= ES8311 DAC output) how long the
//! tone keeps playing afterwards. Resolution: one 2 ms block, plus the I2S RX
//! DMA granularity (~15 ms) common to both cases.
//!
//! `cargo run --release --example barge_in`

#[path = "common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::{rms, spawn_pinned, Synth};
use esp32_s3_korvo2::{Korvo2, Mic};

const BLOCK: usize = 32; // 2 ms at 16 kHz

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let board = Korvo2::take()?;
    let (mut mic, mut speaker) = (board.mic, board.speaker);
    speaker.set_volume(40)?;
    let control = speaker.control();
    let playing = Arc::new(AtomicBool::new(false));

    let p = playing.clone();
    spawn_pinned(c"player", 6, None, 8 * 1024, move || {
        let mut synth = Synth::new(speaker.sample_rate());
        let mut out = vec![0i16; 320];
        loop {
            if p.load(Ordering::Acquire) {
                synth.tone(&mut out, 1000.0, 0.3);
                let _ = speaker.write(&out);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    })?;

    let mut buf = vec![0i16; BLOCK * Mic::CHANNELS];
    let mut read_ms = |mic: &mut Mic, ms: usize| -> anyhow::Result<f32> {
        let mut acc = 0f32;
        for _ in 0..ms / 2 {
            mic.read(&mut buf)?;
            acc = acc.max(rms(&buf, Mic::CHANNELS, 2));
        }
        Ok(acc)
    };

    let (mut with, mut without) = (Vec::new(), Vec::new());
    for round in 0..10 {
        let use_clear = round % 2 == 0;
        playing.store(true, Ordering::Release);
        read_ms(&mut mic, 1000)?;
        let level = read_ms(&mut mic, 100)?;
        // Interrupt.
        playing.store(false, Ordering::Release);
        if use_clear {
            control.clear()?;
        }
        // Count 2 ms blocks until the reference drops below 5 % of the tone level.
        let mut blocks = 0usize;
        loop {
            let r = read_ms(&mut mic, 2)?;
            if r < level * 0.05 || blocks > 250 {
                break;
            }
            blocks += 1;
        }
        let ms = blocks * 2;
        log::info!(
            "round {round}: {} -> tone audible for {ms} ms after stop (ref level {level:.0})",
            if use_clear { "clear()" } else { "no clear" }
        );
        if use_clear {
            with.push(ms);
        } else {
            without.push(ms);
        }
        read_ms(&mut mic, 400)?;
    }
    let avg = |v: &[usize]| v.iter().sum::<usize>() as f32 / v.len().max(1) as f32;
    log::info!(
        "RESULT residual playback after stop: with clear() avg {:.0} ms (max {}), without avg {:.0} ms (max {})",
        avg(&with),
        with.iter().max().unwrap_or(&0),
        avg(&without),
        without.iter().max().unwrap_or(&0)
    );
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
