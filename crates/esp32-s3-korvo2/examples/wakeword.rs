// SPDX-License-Identifier: MIT OR Apache-2.0
//! Standalone WakeNet (no AFE) on the raw MIC1 channel.
//!
//! Uses `wn9_jarvis_tts` ("Jarvis") when present in the model partition, else
//! `wn9_hiesp` ("Hi ESP"), else the first WakeNet model found. The red LED
//! blinks on each detection.
//!
//! `cargo run --release --example wakeword` (models must be flashed, see README)

use std::time::Duration;

use esp32_s3_korvo2::{Korvo2, Led, Mic};
use esp_afe::wakenet::{DetMode, ModelList, WakeNet};

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let board = Korvo2::take()?;
    let (mut mic, leds) = (board.mic, board.leds);

    let models = ModelList::load("model")?;
    let names = models.wakenet_names();
    log::info!("WakeNet models: {names:?}");
    let model = ["wn9_jarvis_tts", "wn9_hiesp"]
        .into_iter()
        .find(|m| models.contains(m))
        .map(str::to_owned)
        .or_else(|| names.first().cloned())
        .ok_or_else(|| anyhow::anyhow!("no WakeNet model in the model partition"))?;

    let mut wn = WakeNet::new(&models, &model, DetMode::Aggressive)?;
    log::info!(
        "{model}: words {:?}, chunk {} x {} ch @ {} Hz, threshold {:.3}",
        (1..=wn.word_count())
            .filter_map(|i| wn.word_name(i))
            .collect::<Vec<_>>(),
        wn.chunk_size(),
        wn.channel_num(),
        wn.sample_rate()?,
        wn.threshold(1)?
    );
    log::info!("say \"{}\"...", wn.word_name(1).unwrap_or("?"));

    let mut frames = vec![0i16; wn.chunk_size() * Mic::CHANNELS];
    let mut mono = vec![0i16; wn.chunk_len()];
    let mut count = 0u32;
    loop {
        mic.read(&mut frames)?;
        for (m, f) in mono.iter_mut().zip(frames.chunks_exact(Mic::CHANNELS)) {
            *m = f[0]; // MIC1
        }
        if let Some(w) = wn.detect(&mono)? {
            count += 1;
            log::info!(
                "WAKE WORD #{count}: \"{}\" (word {}, ~{} ms)",
                w.name,
                w.index,
                w.length_samples / 16
            );
            wn.clean()?;
            leds.set(Led::Red, true)?;
            std::thread::sleep(Duration::from_millis(300));
            leds.set(Led::Red, false)?;
        }
    }
}
