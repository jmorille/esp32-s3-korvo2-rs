// SPDX-License-Identifier: MIT OR Apache-2.0
//! Wi-Fi audio streaming / latency demo (generic, no application protocol).
//!
//! * Wi-Fi station via esp-idf-svc, power save **off** for low latency.
//! * SNTP time sync (logged).
//! * Server discovery with mDNS (`_jarvis._tcp`), falling back to `STREAM_URL`.
//! * WebSocket client:
//!   - **send**: AFE output (AEC + NS, 16 kHz mono `i16` little endian) as binary
//!     frames of 20 ms (320 samples);
//!   - **receive**: binary PCM frames (same format) played on the speaker through
//!     a small jitter buffer;
//!   - a text `ping <micros>` every second; the server must echo it back, the
//!     round trip time is logged every 5 s with the audio counters.
//!
//! Test without any gateway using the echo server in `tools/ws_echo_server.py`
//! (it sends your processed voice back, so you hear yourself ~RTT later).
//!
//! Credentials are **never** stored in the repository. Provide them at build time:
//!
//! ```text
//! WIFI_SSID=myssid WIFI_PASS=secret STREAM_URL=ws://192.168.1.10:8765/ \
//!     cargo run --release -p korvo2-demos --example wifi_stream
//! ```
//!
//! or at runtime in NVS (namespace `wifi_stream`, keys `ssid`, `pass`, `url`),
//! which take precedence over the build-time values.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use esp32_s3_korvo2::{Korvo2, Led, Mic};
use esp_afe::{AecConfig, AecMode, AecNlpLevel, Afe, AfeConfig, AfeType, Models, VadConfig};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::mdns::{EspMdns, QueryResult};
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_svc::sntp::{EspSntp, SyncStatus};
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::ws::client::{
    EspWebSocketClient, EspWebSocketClientConfig, FrameType, WebSocketEventType,
};

/// 20 ms at 16 kHz.
const FRAME: usize = 320;
/// Playback starts once this much audio is buffered (jitter buffer depth).
const PREBUFFER: usize = 3 * FRAME;
/// Drop the oldest audio above this level to bound latency.
const MAX_BUFFER: usize = 25 * FRAME;

#[derive(Default)]
struct Stats {
    sent_frames: AtomicU32,
    recv_frames: AtomicU32,
    underruns: AtomicU32,
    dropped: AtomicU32,
    rtt_us_sum: AtomicU32,
    rtt_us_min: AtomicU32,
    rtt_us_max: AtomicU32,
    rtt_n: AtomicU32,
}

struct Jitter {
    buf: Mutex<VecDeque<i16>>,
    cv: Condvar,
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let nvs_part = EspDefaultNvsPartition::take()?;

    // --- credentials: NVS overrides build-time env ----------------------------
    let nvs = EspNvs::<NvsDefault>::new(nvs_part.clone(), "wifi_stream", true)?;
    let get = |key: &str, fallback: Option<&'static str>| -> Option<String> {
        let mut buf = [0u8; 128];
        nvs.get_str(key, &mut buf)
            .ok()
            .flatten()
            .map(str::to_owned)
            .or_else(|| fallback.map(str::to_owned))
            .filter(|s| !s.is_empty())
    };
    let ssid = get("ssid", option_env!("WIFI_SSID"));
    let pass = get("pass", option_env!("WIFI_PASS")).unwrap_or_default();
    let url_fallback = get("url", option_env!("STREAM_URL"));
    let Some(ssid) = ssid else {
        anyhow::bail!("no Wi-Fi credentials: build with WIFI_SSID/WIFI_PASS or set them in NVS");
    };
    let _wifi = connect_wifi(peripherals.modem, sysloop, nvs_part, &ssid, &pass)?;
    stream(url_fallback)
}

fn connect_wifi(
    modem: esp_idf_svc::hal::modem::Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs_part: EspDefaultNvsPartition,
    ssid: &str,
    pass: &str,
) -> anyhow::Result<BlockingWifi<EspWifi<'static>>> {
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(nvs_part))?,
        sysloop,
    )?;
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID too long"))?,
        password: pass
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method: if pass.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    }))?;
    wifi.start()?;
    // Power save off: the default modem sleep adds tens of ms of latency.
    // SAFETY: plain ESP-IDF call after esp_wifi_start().
    esp_idf_svc::sys::esp!(unsafe {
        esp_idf_svc::sys::esp_wifi_set_ps(esp_idf_svc::sys::wifi_ps_type_t_WIFI_PS_NONE)
    })?;
    wifi.connect()?;
    wifi.wait_netif_up()?;
    let ip = wifi.wifi().sta_netif().get_ip_info()?;
    log::info!("Wi-Fi up: {} (power save off)", ip.ip);
    Ok(wifi)
}

fn stream(url_fallback: Option<String>) -> anyhow::Result<()> {
    // --- SNTP -----------------------------------------------------------------
    let sntp = EspSntp::new_default()?;
    let t0 = Instant::now();
    while sntp.get_sync_status() != SyncStatus::Completed && t0.elapsed() < Duration::from_secs(10)
    {
        std::thread::sleep(Duration::from_millis(200));
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    log::info!(
        "SNTP {:?}: unix time {}.{:03}",
        sntp.get_sync_status(),
        now.as_secs(),
        now.subsec_millis()
    );

    // --- discovery --------------------------------------------------------------
    let url = discover().or(url_fallback).ok_or_else(|| {
        anyhow::anyhow!("no _jarvis._tcp service found and no STREAM_URL configured")
    })?;
    log::info!("stream URL: {url}");

    // --- audio ------------------------------------------------------------------
    let board = Korvo2::take()?;
    let (mut mic, mut speaker, leds) = (board.mic, board.speaker, board.leds);
    speaker.set_volume(50)?;
    let models = Models::load("model").ok();
    let vad_model = models.as_ref().and_then(|m| m.find("vadnet", None));
    let cfg = AfeConfig::new(Mic::AFE_FORMAT)?
        .afe_type(AfeType::FullDuplex)
        .aec(AecConfig {
            mode: AecMode::FdLowCost,
            nlp_level: AecNlpLevel::VeryAggressive,
            filter_length: None,
        })
        .ns(true)
        .no_wakenet()
        .vad(VadConfig {
            model: vad_model,
            ..Default::default()
        })
        .task(0, 5);
    let afe = Afe::new(&cfg, models.as_ref())?;
    let (mut feeder, mut fetcher) = afe.split();

    let stats = Arc::new(Stats {
        rtt_us_min: AtomicU32::new(u32::MAX),
        ..Default::default()
    });
    let jitter = Arc::new(Jitter {
        buf: Mutex::new(VecDeque::with_capacity(MAX_BUFFER)),
        cv: Condvar::new(),
    });
    let start = Instant::now();

    // --- WebSocket ----------------------------------------------------------------
    let (st, jb) = (stats.clone(), jitter.clone());
    let ws_cfg = EspWebSocketClientConfig {
        reconnect_timeout_ms: Duration::from_secs(2),
        network_timeout_ms: Duration::from_secs(5),
        buffer_size: 4096,
        ..Default::default()
    };
    let client = EspWebSocketClient::new(&url, &ws_cfg, Duration::from_secs(10), move |ev| {
        match ev {
            Ok(ev) => match ev.event_type {
                WebSocketEventType::Binary(data) => {
                    st.recv_frames.fetch_add(1, Ordering::Relaxed);
                    let mut b = jb.buf.lock().unwrap();
                    b.extend(
                        data.chunks_exact(2)
                            .map(|c| i16::from_le_bytes([c[0], c[1]])),
                    );
                    if b.len() > MAX_BUFFER {
                        let n = b.len() - MAX_BUFFER;
                        b.drain(..n);
                        st.dropped.fetch_add(n as u32, Ordering::Relaxed);
                    }
                    jb.cv.notify_one();
                }
                WebSocketEventType::Text(t) => {
                    if let Some(sent) = t.strip_prefix("ping ").and_then(|v| v.parse::<u32>().ok())
                    {
                        // Microsecond timestamps wrap after ~71 min: wrapping arithmetic.
                        let rtt = micros(start).wrapping_sub(sent);
                        st.rtt_us_sum.fetch_add(rtt, Ordering::Relaxed);
                        // Note: AtomicU32::fetch_min/fetch_max trip an Xtensa LLVM bug
                        // ("Undefined temporary symbol"), so use fetch_update.
                        let _ =
                            st.rtt_us_min
                                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |m| {
                                    (rtt < m).then_some(rtt)
                                });
                        let _ =
                            st.rtt_us_max
                                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |m| {
                                    (rtt > m).then_some(rtt)
                                });
                        st.rtt_n.fetch_add(1, Ordering::Relaxed);
                    }
                }
                WebSocketEventType::Connected => log::info!("WebSocket connected"),
                WebSocketEventType::Disconnected => log::warn!("WebSocket disconnected"),
                _ => {}
            },
            Err(e) => log::warn!("WebSocket error: {e}"),
        }
    })?;
    let client = Arc::new(Mutex::new(client));

    // Mic -> AFE (core 0, high priority: never miss I2S data).
    spawn(c"afe-feed", 10, Some(Core::Core0), 8 * 1024, move || {
        let mut buf = vec![0i16; feeder.chunk_len()];
        loop {
            if mic.read(&mut buf).is_ok() {
                let _ = feeder.feed(&buf);
            }
        }
    })?;

    // AFE -> WebSocket in 20 ms binary frames (core 1).
    let (c, st) = (client.clone(), stats.clone());
    spawn(c"ws-send", 8, Some(Core::Core1), 16 * 1024, move || {
        let mut pending: Vec<i16> = Vec::with_capacity(4 * FRAME);
        let mut bytes = Vec::with_capacity(2 * FRAME);
        loop {
            let Ok(frame) = fetcher.fetch() else { continue };
            pending.extend_from_slice(&frame.data);
            while pending.len() >= FRAME {
                bytes.clear();
                for s in pending.drain(..FRAME) {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let mut cl = c.lock().unwrap();
                if cl.is_connected() && cl.send(FrameType::Binary(false), &bytes).is_ok() {
                    st.sent_frames.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    })?;

    // Jitter buffer -> speaker.
    let (jb, st) = (jitter.clone(), stats.clone());
    spawn(c"player", 7, Some(Core::Core0), 8 * 1024, move || {
        let mut out = vec![0i16; FRAME];
        let mut playing = false;
        loop {
            {
                let mut b = jb.buf.lock().unwrap();
                if !playing {
                    while b.len() < PREBUFFER {
                        b = jb.cv.wait(b).unwrap();
                    }
                    playing = true;
                }
                if b.len() >= FRAME {
                    for (o, s) in out.iter_mut().zip(b.drain(..FRAME)) {
                        *o = s;
                    }
                } else {
                    out.fill(0);
                    st.underruns.fetch_add(1, Ordering::Relaxed);
                    playing = false;
                }
            }
            let _ = speaker.write(&out);
        }
    })?;

    // Ping + statistics (main task).
    let mut last = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let connected = {
            let mut cl = client.lock().unwrap();
            let msg = format!("ping {}", micros(start));
            cl.is_connected() && cl.send(FrameType::Text(false), msg.as_bytes()).is_ok()
        };
        leds.set(Led::Blue, connected)?;
        if last.elapsed() >= Duration::from_secs(5) {
            last = Instant::now();
            let n = stats.rtt_n.swap(0, Ordering::Relaxed);
            let sum = stats.rtt_us_sum.swap(0, Ordering::Relaxed);
            let min = stats.rtt_us_min.swap(u32::MAX, Ordering::Relaxed);
            let max = stats.rtt_us_max.swap(0, Ordering::Relaxed);
            let buffered = jitter.buf.lock().unwrap().len();
            log::info!(
                "sent {} recv {} frames | jitter buf {} ms, underruns {}, dropped {} | RTT {}",
                stats.sent_frames.swap(0, Ordering::Relaxed),
                stats.recv_frames.swap(0, Ordering::Relaxed),
                buffered / 16,
                stats.underruns.swap(0, Ordering::Relaxed),
                stats.dropped.swap(0, Ordering::Relaxed),
                if n == 0 {
                    "n/a".to_owned()
                } else {
                    format!(
                        "min {:.1} / avg {:.1} / max {:.1} ms (n={n})",
                        min as f32 / 1000.0,
                        sum as f32 / n as f32 / 1000.0,
                        max as f32 / 1000.0
                    )
                }
            );
        }
    }
}

/// Microseconds since `start`, wrapping at 2^32 (avoids u128 arithmetic).
fn micros(start: Instant) -> u32 {
    let e = start.elapsed();
    (e.as_secs() as u32)
        .wrapping_mul(1_000_000)
        .wrapping_add(e.subsec_micros())
}

/// Look up `_jarvis._tcp` with mDNS and build a `ws://` URL from the first answer.
/// The optional TXT record `path` gives the URL path (default `/`).
fn discover() -> Option<String> {
    let mdns = EspMdns::take().ok()?;
    let mut results: [QueryResult; 1] = [QueryResult {
        instance_name: None,
        hostname: None,
        port: 0,
        txt: Vec::new(),
        addr: Vec::new(),
        interface: esp_idf_svc::mdns::Interface::STA,
        ip_protocol: esp_idf_svc::mdns::Protocol::V4,
    }];
    let n = mdns
        .query_ptr("_jarvis", "_tcp", Duration::from_secs(3), 1, &mut results)
        .ok()?;
    if n == 0 {
        log::info!("mDNS: no _jarvis._tcp service");
        return None;
    }
    let r = &results[0];
    let addr = r.addr.first()?;
    let path = r
        .txt
        .iter()
        .find(|(k, _)| k == "path")
        .map(|(_, v)| v.clone())
        .unwrap_or_else(|| "/".into());
    log::info!(
        "mDNS: found {:?} on {:?} {addr}:{}",
        r.instance_name,
        r.hostname,
        r.port
    );
    Some(format!("ws://{addr}:{}{path}", r.port))
}

fn spawn<F>(
    name: &'static core::ffi::CStr,
    priority: u8,
    core: Option<Core>,
    stack_size: usize,
    f: F,
) -> anyhow::Result<()>
where
    F: FnOnce() + Send + 'static,
{
    ThreadSpawnConfiguration {
        name: Some(name),
        stack_size,
        priority,
        pin_to_core: core,
        ..Default::default()
    }
    .set()?;
    std::thread::Builder::new()
        .stack_size(stack_size)
        .spawn(f)?;
    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}
