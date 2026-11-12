// SPDX-License-Identifier: MIT OR Apache-2.0
//! ESP-SR AFE with AEC + NS + VAD on the Korvo-2, with an echo test.
//!
//! The speaker plays a speech-like test signal in some phases. For each 5 s
//! phase the example logs the share of frames the VAD classified as speech:
//!
//! | phase           | playback | AEC | expected speech ratio            |
//! |-----------------|----------|-----|----------------------------------|
//! | `quiet`         | no       | on  | ~0 % (unless you talk)           |
//! | `play+aec`      | yes      | on  | low: echo removed before the VAD |
//! | `quiet`         | no       | on  | ~0 %                             |
//! | `play-noaec`    | yes      | off | high: the echo looks like speech |
//!
//! VAD transitions are logged as they happen, so you can also talk over the
//! playback and check that your voice is still detected (barge-in).
//!
//! `cargo run --release --example afe_vad` (models must be flashed, see README)

#[path = "common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::Synth;
use esp32_s3_korvo2::{Korvo2, Led, Mic};
use esp_afe::{
    AecConfig, AecMode, AecNlpLevel, Afe, AfeConfig, AfeType, Feature, Models, Vad, VadConfig,
    VadMode,
};

const PHASE: Duration = Duration::from_secs(5);
const PHASES: [(&str, bool, bool); 4] = [
    ("quiet", false, true),
    ("play+aec", true, true),
    ("quiet", false, true),
    ("play-noaec", true, false),
];

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let board = Korvo2::take()?;
    let (mut mic, mut speaker, leds) = (board.mic, board.speaker, board.leds);
    speaker.set_volume(60)?;
    // Render the test signal before the AFE starts (takes a few seconds).
    let mut synth = Synth::new(speaker.sample_rate());
    synth.prepare_babble(0.5);

    let models = Models::load("model").ok();
    if let Some(m) = &models {
        log::info!("models in partition: {:?}", m.names());
    } else {
        log::warn!("no model partition: using WebRTC VAD");
    }
    let vad_model = models.as_ref().and_then(|m| m.find("vadnet", None));
    log::info!("VAD model: {}", vad_model.as_deref().unwrap_or("WebRTC"));

    let cfg = AfeConfig::new(Mic::AFE_FORMAT)?
        .afe_type(AfeType::FullDuplex)
        .aec(AecConfig {
            mode: AecMode::FdLowCost,
            nlp_level: AecNlpLevel::VeryAggressive,
            filter_length: None,
        })
        .ns(true)
        .no_wakenet()
        .no_agc()
        .vad(VadConfig {
            mode: VadMode::Mode1,
            min_speech_ms: 128,
            min_noise_ms: 500,
            model: vad_model,
        });
    let afe = Afe::new(&cfg, models.as_ref())?;
    afe.print_pipeline()?;
    let info = afe.info();
    log::info!("AFE: {info:?}");
    let (mut feeder, mut fetcher) = afe.split();

    // Current phase index, shared with the player and feeder threads.
    let phase = Arc::new(AtomicU8::new(0));

    // Feeder: microphones -> AFE.
    std::thread::Builder::new()
        .name("afe-feed".into())
        .stack_size(8 * 1024)
        .spawn(move || {
            let mut buf = vec![0i16; feeder.chunk_len()];
            loop {
                if let Err(e) = mic
                    .read(&mut buf)
                    .map_err(anyhow::Error::from)
                    .and_then(|_| feeder.feed(&buf).map_err(anyhow::Error::from))
                {
                    log::error!("feed: {e}");
                }
            }
        })?;

    // Player: test signal during the "play" phases, silence otherwise.
    let ph = phase.clone();
    std::thread::Builder::new()
        .name("player".into())
        .stack_size(8 * 1024)
        .spawn(move || {
            let mut out = vec![0i16; 256];
            loop {
                let (_, play, _) = PHASES[usize::from(ph.load(Ordering::Relaxed))];
                if play {
                    synth.babble(&mut out, 0.5);
                } else {
                    out.fill(0);
                }
                if let Err(e) = speaker.write(&out) {
                    log::error!("speaker: {e}");
                }
            }
        })?;

    // Fetcher: AFE output, VAD statistics.
    let mut idx = 0usize;
    let mut started = Instant::now();
    let (mut speech, mut total) = (0u32, 0u32);
    let mut energy = 0f32;
    let mut last_vad = Vad::Silence;
    loop {
        let frame = match fetcher.fetch() {
            Ok(f) => f,
            Err(e) => {
                log::warn!("fetch: {e}");
                continue;
            }
        };
        total += 1;
        energy += 10f32.powf(frame.volume_db / 10.0);
        if frame.vad == Vad::Speech {
            speech += 1;
        }
        if frame.vad != last_vad {
            log::info!(
                "[{:10}] VAD -> {:?} (input {:.1} dBFS)",
                PHASES[idx].0,
                frame.vad,
                frame.volume_db
            );
            leds.set(Led::Blue, frame.vad == Vad::Speech)?;
            last_vad = frame.vad;
        }
        if started.elapsed() >= PHASE {
            log::info!(
                "== phase {:10} playback={} aec={}: speech {:5.1} % of {} frames, mean VAD input {:.1} dBFS",
                PHASES[idx].0,
                PHASES[idx].1,
                PHASES[idx].2,
                100.0 * speech as f32 / total.max(1) as f32,
                total,
                10.0 * (energy / total.max(1) as f32).max(1e-12).log10()
            );
            let prev_aec = PHASES[idx].2;
            idx = (idx + 1) % PHASES.len();
            if PHASES[idx].2 != prev_aec {
                fetcher.set_enabled(Feature::Aec, PHASES[idx].2)?;
            }
            phase.store(idx as u8, Ordering::Relaxed);
            leds.set(Led::Red, PHASES[idx].1)?;
            started = Instant::now();
            (speech, total, energy) = (0, 0, 0.0);
        }
    }
}
