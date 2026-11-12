// SPDX-License-Identifier: MIT OR Apache-2.0
//! Compares AFE configurations for a full-duplex voice satellite: AEC + VADNet
//! + WakeNet together, while the speaker plays speech-like audio.
//!
//! For each configuration (15 s after a 3 s warm-up) it logs: CPU load per core (FreeRTOS
//! run-time stats), the AFE ring buffer free ratio (min / average), fetch
//! timeouts, the share of frames flagged as speech (echo leaking through the
//! AEC into the VAD) and wake word detections.
//!
//! The AFE is created on the main task: with ESP-SR 2.6.2, creating it from a
//! spawned `std::thread` crashed in `afe_create_from_config` (see README).
//!
//! Needs `CONFIG_FREERTOS_USE_TRACE_FACILITY` and
//! `CONFIG_FREERTOS_GENERATE_RUN_TIME_STATS` (set in `sdkconfig.defaults`).
//!
//! `cargo run --release --example afe_bench`

#[path = "common/mod.rs"]
mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{spawn_pinned, Synth};
use esp32_s3_korvo2::{Korvo2, Mic};
use esp_afe::{
    AecConfig, AecMode, AecNlpLevel, Afe, AfeConfig, AfeMode, AfeType, DetMode, Models, Vad,
    VadConfig,
};
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::sys;

const RUN: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq)]
enum VadKind {
    Off,
    VadNet,
}

/// One AFE configuration under test.
struct Combo {
    name: &'static str,
    afe_type: AfeType,
    aec_mode: AecMode,
    nlp: AecNlpLevel,
    vad: VadKind,
    wakenet: bool,
}

/// Configurations compared (results in the README).
const COMBOS: [Combo; 5] = [
    Combo {
        name: "FD+FD_LOW_COST, VADNet, WN",
        afe_type: AfeType::FullDuplex,
        aec_mode: AecMode::FdLowCost,
        nlp: AecNlpLevel::VeryAggressive,
        vad: VadKind::VadNet,
        wakenet: true,
    },
    Combo {
        name: "FD+FD_HIGH_PERF, VADNet, WN",
        afe_type: AfeType::FullDuplex,
        aec_mode: AecMode::FdHighPerf,
        nlp: AecNlpLevel::VeryAggressive,
        vad: VadKind::VadNet,
        wakenet: true,
    },
    Combo {
        name: "SR+SR_LOW_COST, no VAD, WN",
        afe_type: AfeType::SpeechRecognition,
        aec_mode: AecMode::SrLowCost,
        nlp: AecNlpLevel::Aggressive,
        vad: VadKind::Off,
        wakenet: true,
    },
    Combo {
        name: "SR+SR_HIGH_PERF, no VAD, WN",
        afe_type: AfeType::SpeechRecognition,
        aec_mode: AecMode::SrHighPerf,
        nlp: AecNlpLevel::Aggressive,
        vad: VadKind::Off,
        wakenet: true,
    },
    Combo {
        name: "SR+SR_LOW_COST, VADNet, WN",
        afe_type: AfeType::SpeechRecognition,
        aec_mode: AecMode::SrLowCost,
        nlp: AecNlpLevel::Aggressive,
        vad: VadKind::VadNet,
        wakenet: true,
    },
];

/// Idle run time per core and total run time (µs), from FreeRTOS run-time stats.
fn idle_counters() -> ([u64; 2], u64) {
    // SAFETY: plain FreeRTOS queries; the status array is sized from the task
    // count with headroom and only the filled entries are read.
    unsafe {
        let n = sys::uxTaskGetNumberOfTasks() as usize + 8;
        let mut tasks: Vec<sys::TaskStatus_t> = vec![core::mem::zeroed(); n];
        let mut total = 0;
        let got = sys::uxTaskGetSystemState(tasks.as_mut_ptr(), n as _, &mut total) as usize;
        let mut idle = [0u64; 2];
        let mut seen = 0;
        for t in &tasks[..got] {
            let name = core::ffi::CStr::from_ptr(t.pcTaskName).to_bytes();
            if name.starts_with(b"IDLE") {
                let core = match name.last() {
                    Some(b'1') => 1,
                    Some(b'0') => 0,
                    _ => seen.min(1),
                };
                idle[core] = t.ulRunTimeCounter as u64;
                seen += 1;
            }
        }
        (idle, total as u64)
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let board = Korvo2::take()?;
    let (mic, mut speaker) = (board.mic, board.speaker);
    speaker.set_volume(50)?;
    let mut synth = Synth::new(speaker.sample_rate());
    synth.prepare_babble(0.5);
    spawn_pinned(c"player", 4, Some(Core::Core0), 8 * 1024, move || {
        let mut out = vec![0i16; 320];
        loop {
            synth.babble(&mut out, 0.5);
            let _ = speaker.write(&out);
        }
    })?;

    let models = Models::load("model")?;
    let wn = models
        .find("wn9_", Some("jarvis"))
        .or_else(|| models.wakenet_names().first().cloned())
        .ok_or_else(|| anyhow::anyhow!("no WakeNet model"))?;
    let vadnet = models.find("vadnet", None);
    log::info!("WakeNet {wn}, VAD {:?}", vadnet);

    // ESP-SR 2.6.2 crashes in `afe_create_from_config` (garbage AEC mode) when
    // the AFE is created from a spawned pthread, so the bench runs on the main
    // task, raised above the player thread.
    // SAFETY: NULL = the calling task.
    unsafe { sys::vTaskPrioritySet(core::ptr::null_mut(), 8) };
    let mut mic = mic;
    for combo in &COMBOS {
        let name = combo.name;
        log::info!("START {name}");
        match run_one(mic, &models, &wn, vadnet.clone(), combo) {
            Ok((m, r)) => {
                log::info!("RESULT {name:36} | {r}");
                mic = m;
            }
            Err(e) => {
                log::error!("RESULT {name:36} | failed: {e}");
                break;
            }
        }
    }
    log::info!("BENCH DONE");
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn run_one(
    mic: Mic,
    models: &Models,
    wn: &str,
    vadnet: Option<String>,
    combo: &Combo,
) -> anyhow::Result<(Mic, String)> {
    let mut cfg = AfeConfig::new(Mic::AFE_FORMAT)?
        .afe_type(combo.afe_type)
        .mode(AfeMode::LowCost)
        .aec(AecConfig {
            mode: combo.aec_mode,
            nlp_level: combo.nlp,
            filter_length: None,
        })
        .task(0, 5);
    cfg = match combo.vad {
        VadKind::Off => cfg.no_vad(),
        VadKind::VadNet => cfg.vad(VadConfig {
            model: vadnet,
            min_noise_ms: 500,
            ..Default::default()
        }),
    };
    cfg = if combo.wakenet {
        cfg.wakenet(wn).wakenet_mode(DetMode::Aggressive)
    } else {
        cfg.no_wakenet()
    };
    let afe = Afe::new(&cfg, Some(models))?;
    afe.print_pipeline()?;
    let (mut feeder, mut fetcher) = afe.split();

    // The feeder thread owns the Mic and hands it back when stopped.
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<Mic>();
    let s = stop.clone();
    let feed = spawn_pinned(c"afe-feed", 10, Some(Core::Core0), 8 * 1024, move || {
        let mut mic = mic;
        let mut buf = vec![0i16; feeder.chunk_len()];
        while !s.load(Ordering::Relaxed) {
            if mic.read(&mut buf).is_ok() {
                let _ = feeder.feed(&buf);
            }
        }
        let _ = tx.send(mic);
    })?;

    // Warm-up (AEC convergence), then measure.
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(3) {
        let _ = fetcher.fetch();
    }
    let (idle0, total0) = idle_counters();
    let start = Instant::now();
    let (mut frames, mut speech, mut wakes, mut timeouts) = (0u32, 0u32, 0u32, 0u32);
    let (mut rb_min, mut rb_sum) = (1.0f32, 0f32);
    while start.elapsed() < RUN {
        match fetcher.fetch() {
            Ok(f) => {
                frames += 1;
                rb_min = rb_min.min(f.ringbuf_free);
                rb_sum += f.ringbuf_free;
                if f.vad == Vad::Speech {
                    speech += 1;
                }
                if let Some(w) = f.wake_word {
                    wakes += 1;
                    log::info!("wake word {:?}", w.name);
                }
            }
            Err(_) => timeouts += 1,
        }
    }
    let (idle1, total1) = idle_counters();
    let dt = (total1 - total0).max(1) as f32;
    let cpu = |c: usize| 100.0 - 100.0 * (idle1[c] - idle0[c]) as f32 / dt;

    stop.store(true, Ordering::Relaxed);
    feed.join().ok();
    let mic = rx.recv()?;
    drop(fetcher);

    Ok((mic, format!(
        "CPU core0 {:4.1} % core1 {:4.1} % | ringbuf free min {:.2} avg {:.2} | frames {frames} timeouts {timeouts} | speech {:4.1} % | wake {wakes}",
        cpu(0),
        cpu(1),
        rb_min,
        rb_sum / frames.max(1) as f32,
        100.0 * speech as f32 / frames.max(1) as f32
    )))
}
