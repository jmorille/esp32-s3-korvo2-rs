// SPDX-License-Identifier: MIT OR Apache-2.0
//! Wake word through the AFE pipeline (AEC + WakeNet) while the speaker plays.
//!
//! Background "babble" is played on the speaker (toggle with PLAY, volume with
//! VOL+/VOL-). Thanks to the AEC reference channel the wake word is still
//! detected over the playback. On detection the red LED lights for 1 s.
//!
//! `cargo run --release --example afe_wakeword` (models must be flashed, see README)

#[path = "common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicBool, AtomicI16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::{spawn_pinned, Synth};
use esp32_s3_korvo2::{Button, ButtonEvent, Korvo2, Led, Mic};
use esp_afe::{AecConfig, AecMode, AecNlpLevel, Afe, AfeConfig, AfeType, DetMode, Models};
use esp_idf_svc::hal::cpu::Core;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let board = Korvo2::take()?;
    let (mut mic, mut speaker, mut buttons, leds) =
        (board.mic, board.speaker, board.buttons, board.leds);
    speaker.set_volume(50)?;
    // Render the test signal before the AFE starts (takes a few seconds).
    let mut synth = Synth::new(speaker.sample_rate());
    synth.prepare_babble(0.5);

    let models = Models::load("model")?;
    let wn = ["wn9_jarvis_tts", "wn9_hiesp"]
        .into_iter()
        .find(|m| models.contains(m))
        .map(str::to_owned)
        .or_else(|| models.wakenet_names().first().cloned())
        .ok_or_else(|| anyhow::anyhow!("no WakeNet model in the model partition"))?;
    log::info!("WakeNet model {wn}, words {:?}", models.wake_words(&wn));

    let cfg = AfeConfig::new(Mic::AFE_FORMAT)?
        .afe_type(AfeType::SpeechRecognition)
        .aec(AecConfig {
            mode: AecMode::SrLowCost,
            nlp_level: AecNlpLevel::Aggressive,
            filter_length: None,
        })
        .no_vad()
        .wakenet(&wn)
        .wakenet_mode(DetMode::Aggressive)
        .task(0, 5);
    let afe = Afe::new(&cfg, Some(&models))?;
    afe.print_pipeline()?;
    let (mut feeder, mut fetcher) = afe.split();

    let playing = Arc::new(AtomicBool::new(true));
    let volume_delta = Arc::new(AtomicI16::new(0));

    spawn_pinned(c"afe-feed", 10, Some(Core::Core0), 8 * 1024, move || {
        let mut buf = vec![0i16; feeder.chunk_len()];
        loop {
            if mic.read(&mut buf).is_ok() {
                if let Err(e) = feeder.feed(&buf) {
                    log::error!("feed: {e}");
                }
            }
        }
    })?;

    let (p, vd) = (playing.clone(), volume_delta.clone());
    spawn_pinned(c"player", 4, Some(Core::Core0), 8 * 1024, move || {
        let mut out = vec![0i16; 256];
        loop {
            let d = vd.swap(0, Ordering::Relaxed);
            if d != 0 {
                if let Ok(v) = speaker.change_volume(d) {
                    log::info!("volume {v}");
                }
            }
            if p.load(Ordering::Relaxed) {
                synth.babble(&mut out, 0.5);
            } else {
                out.fill(0);
            }
            let _ = speaker.write(&out);
        }
    })?;

    spawn_pinned(c"buttons", 2, None, 4 * 1024, move || loop {
        match buttons.poll() {
            Ok(Some(ButtonEvent::Pressed(b))) => {
                log::info!("button {b:?}");
                match b {
                    Button::Play => {
                        let now = !playing.load(Ordering::Relaxed);
                        playing.store(now, Ordering::Relaxed);
                        log::info!("playback {}", if now { "on" } else { "off" });
                    }
                    Button::VolUp => volume_delta.store(10, Ordering::Relaxed),
                    Button::VolDown => volume_delta.store(-10, Ordering::Relaxed),
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(e) => log::error!("buttons: {e}"),
        }
        std::thread::sleep(Duration::from_millis(20));
    })?;

    log::info!("listening (playback on, PLAY toggles it)...");
    // Detection runs on core 1 at a higher priority than the main task.
    let detector = spawn_pinned(c"afe-fetch", 8, Some(Core::Core1), 16 * 1024, move || {
        if let Err(e) = detect(&mut fetcher, &leds) {
            log::error!("detector: {e}");
        }
    })?;
    detector.join().ok();
    Ok(())
}

fn detect(fetcher: &mut esp_afe::AfeFetcher, leds: &esp32_s3_korvo2::Leds) -> anyhow::Result<()> {
    let mut count = 0u32;
    let mut led_off_in = 0u32;
    loop {
        let frame = match fetcher.fetch() {
            Ok(f) => f,
            Err(e) => {
                log::warn!("fetch: {e}");
                continue;
            }
        };
        if let Some(w) = &frame.wake_word {
            count += 1;
            log::info!(
                "WAKE WORD #{count}: \"{}\" (model {}, word {}, {} samples)",
                w.name,
                w.model_index,
                w.index,
                w.length_samples
            );
            leds.set(Led::Red, true)?;
            led_off_in = 30; // ~1 s of 32 ms frames
        }
        if led_off_in > 0 {
            led_off_in -= 1;
            if led_off_in == 0 {
                leds.set(Led::Red, false)?;
            }
        }
    }
}
