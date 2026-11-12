# esp32-s3-korvo2-rs

[![CI](https://github.com/jmorille/esp32-s3-korvo2-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/jmorille/esp32-s3-korvo2-rs/actions/workflows/ci.yml)

Rust (`std`, ESP-IDF) support for the Espressif **ESP32-S3-Korvo-2 V3.x** voice
development board and the **ESP-SR** speech front-end.

| Crate | What it is |
|---|---|
| [`esp-afe`](crates/esp-afe) | Safe wrapper for ESP-SR on any ESP32-S3: Audio Front-End (AEC, BSS, NS, VAD/VADNet, AGC, WakeNet) with a config builder and `Send` feeder/fetcher halves, a standalone `WakeNet` detector, and the model partition. |
| [`esp32-s3-korvo2`](crates/esp32-s3-korvo2) | Board support: ES8311 (speaker) + ES7210 (2 mics + AEC reference) over I2S0 full duplex via `esp_codec_dev`, volume / mute / gains, 6 ADC buttons with debounced events, LEDs on the I/O expander, battery voltage. |
| [`korvo2-demos`](crates/korvo2-demos) | Not published. Networked demo (`wifi_stream`); also the esp-idf-sys "root crate" of this workspace. |

Built on [`esp-idf-svc`](https://github.com/esp-rs/esp-idf-svc) 0.53 / `esp-idf-sys` 0.38,
ESP-IDF **v5.5**, target `xtensa-esp32s3-espidf`. ESP-SR (`espressif/esp-sr` 2.6) and
`espressif/esp_codec_dev` 1.6 are fetched from the Espressif component registry at
build time and bound with bindgen.

## Features

* `Korvo2::take()` → `Mic`, `Speaker`, `Buttons`, `Battery`, `Leds`, each `Send` so
  they can live on different threads.
* `Mic::read()` yields interleaved frames **already ordered for the AFE `"MMR"`
  layout**: `[MIC1, MIC2, REF]` (the ES7210 TDM order is MIC1, MIC3, MIC2, MIC4; MIC3
  carries the ES8311 DAC output, i.e. the echo reference).
* `Speaker::write()` plays 16 kHz mono `i16`; `set_volume`, `change_volume`, `set_mute`.
* `esp_afe::Afe` with `AfeConfig::new("MMR")…` builder → `split()` into `AfeFeeder` /
  `AfeFetcher`; `AfeFrame { data, vad, wake_word, volume_db, … }`; runtime
  `enable_wakenet()` / `disable_wakenet()` / `set_enabled(Feature::Aec, …)`.
* `esp_afe::wakenet::WakeNet` for wake word detection without the AFE.
* `esp_afe::Models` lists the models of the `model` partition (`wakenet_names()`,
  `wake_words()`, `find()`); pure-Rust parsing helpers in `esp_afe::model_info`
  (host-testable).
* All `unsafe` code is confined to one private module per crate (`raw` / `ffi`),
  each block with a `SAFETY:` comment.

## Hardware notes

* Plug **both USB-C cables**: *USB Power* (5 V / 2 A) **and** *USB-to-UART*
  (flashing / logs), then put the **Power switch ON**. Connect a speaker.
* Download mode is automatic (DTR/RTS); manually: hold Boot, press Reset.
* Module: ESP32-S3-WROOM-1 **N16R8**: 16 MB quad flash, 8 MB **octal** PSRAM
  (`CONFIG_SPIRAM_MODE_OCT=y`, see `sdkconfig.defaults`).

### Pins (V3.1 schematic, cross-checked with Espressif's reference board code)

| Function | GPIO | Notes |
|---|---|---|
| I2C SDA / SCL | 17 / 18 | ES8311 `0x18`, ES7210 `0x40`, I/O expander `0x20` (TCA9554) or `0x38` (TCA9554A) |
| I2S0 MCLK | 16 | 256 × fs |
| I2S0 BCLK / WS | 9 / 45 | shared by both codecs |
| I2S0 DOUT → ES8311 | 8 | standard (Philips) mode |
| I2S0 DIN ← ES7210 | 10 | 4-slot TDM |
| PA enable (NS4150) | 48 | high = on, driven by esp_codec_dev |
| Buttons (ADC1 ch4) | 5 | REC 2.41 V, MUTE 1.98, PLAY 1.65, SET 1.11, VOL- 0.82, VOL+ 0.38 |
| Battery (ADC1 ch5) | 6 | divider 301 k / 100 k → ×4.01 |
| Expander P5 / P6 / P7 | — | PERI_PWR_ON (low = on) / red LED / blue LED |

## Quick start

```sh
# 1. Toolchain (Xtensa Rust fork) + tools
cargo install espup ldproxy espflash
espup install --targets esp32s3       # then: . ~/export-esp.sh
# 2. Build all examples (the first build downloads ESP-IDF into .embuild/, ~10 min)
cargo build --release --workspace --examples
# 3. Flash the ESP-SR models once (see below), then run an example
cargo run --release --example loopback
```

`cargo run` uses the runner from `.cargo/config.toml`:
`espflash flash --monitor --chip esp32s3 --flash-size 16mb --partition-table partitions.csv`.

### Examples

| Example | Shows | Verified on hardware |
|---|---|---|
| `loopback` | RMS of MIC1, MIC2 and REF in silence / 1 kHz tone / mic→speaker phases; battery voltage; button events | yes |
| `afe_vad` | AFE with AEC + NS + VADNet; plays a speech-like signal and logs the VAD speech ratio and residual level with AEC on vs off | yes |
| `wakeword` | Standalone `WakeNet` on the raw MIC1 channel ("Jarvis", else "Hi ESP") | runs, detection not verified |
| `afe_wakeword` | Wake word through the AFE with AEC while the speaker plays; PLAY / VOL± buttons | runs, detection not verified |
| `barge_in` | Residual playback after stopping a tone, with and without `SpeakerControl::clear()` | yes |
| `afe_bench` | CPU load, AFE ring buffer, VAD false positives and wake detections for several AFE / AEC combinations during playback | yes |
| `wifi_stream` (`-p korvo2-demos`) | Wi-Fi + mDNS + WebSocket full-duplex PCM streaming with RTT stats | compiled only |

```sh
cargo run --release --example afe_vad
cargo run --release --example wakeword
WIFI_SSID=… WIFI_PASS=… STREAM_URL=ws://192.168.1.10:8765/ \
  cargo run --release -p korvo2-demos --example wifi_stream
```

Measured with `afe_vad` on a V3.1 board (volume 60, after AEC convergence):
speech-like playback gives ≈ 90–100 % "speech" frames with AEC off and **0 %**
with AEC on (FD low-cost mode, NLP very aggressive), residual level −62 dBFS vs
−49 dBFS without AEC. The first seconds of playback after boot are still detected
while the AEC filter converges.

### AFE configurations for a full-duplex satellite (`afe_bench`)

AEC on, WakeNet `wn9_jarvis_tts` (aggressive), AFE task on core 0 (priority 5),
speech-like playback at volume 50, 15 s after a 3 s warm-up, V3.1 board:

| AFE type + AEC mode | VAD | CPU core 0 | ring buffer free (min / avg) | echo flagged as speech |
|---|---|---|---|---|
| FD + `FdLowCost`, NLP very aggressive | VADNet | 85 % | 0.98 / 0.99 | **0 %** |
| FD + `FdHighPerf`, NLP very aggressive | VADNet | 99 % | 0.94 / 0.99 | 8 % |
| SR + `SrLowCost` | none | 55 % | 0.98 / 0.99 | – |
| SR + `SrHighPerf` | none | 68 % | 0.98 / 0.99 | – |
| SR + `SrLowCost` | VADNet | 72 % | 0.98 / 0.99 | 73 % |

No fetch timeout and no ring buffer overflow in any configuration; core 1 stays
idle (the fetch loop only waits). SR + `SrLowCost` with AEC + VADNet + WakeNet
**works**, but its AEC leaves enough echo for the VAD to flag ~73 % of playback
frames as speech, so VAD-based barge-in is unusable with it. **Recommended for a
full-duplex satellite: `AfeType::FullDuplex` + `AecMode::FdLowCost` +
`AecNlpLevel::VeryAggressive` + VADNet + WakeNet** (0 % false speech, 15 % CPU
headroom on core 0; keep other heavy work on core 1). SR + `SrLowCost` without VAD
is the cheapest option for wake-word-only use.

> **Create the AFE on the main task.** With ESP-SR 2.6.2, `Afe::new` called from
> a spawned `std::thread` crashed in `afe_create_from_config`
> ("Unsupported AEC mode for AEC3 path", then `LoadProhibited`) for every
> configuration, while the same configurations work when created from `main`.
> Feeding and fetching from other threads is fine. The root cause is inside the
> closed ESP-SR library and was not found.

## Barge-in: `SpeakerControl::clear()`

`Speaker::write` queues audio in 20 ms chunks; `speaker.control()` returns a
`Clone + Send` handle whose `clear()` disables the I2S TX channel, preloads its DMA
buffers with silence (`i2s_channel_preload_data`) and re-enables it, and makes a
`write` in progress on another thread return after its current chunk. The
microphones keep running.

Measured with `barge_in` (16 kHz, tone observed on the AEC reference channel):

| Stop method | Audible after stop (avg / max) |
|---|---|
| `clear()` | **27 ms / 28 ms** |
| stop calling `write` only | 119 ms / 120 ms |

Both include up to ~15 ms of RX DMA buffering on the measuring side.

## Sample rate

`Config::sample_rate` accepts 16000 (default), 22050, 24000, 32000 and 48000 Hz.
The ES8311 (speaker) and ES7210 (mics) share MCLK, BCLK and WS on the board, so
**playback and capture always run at the same rate**: different TX/RX rates are
not possible. ESP-SR needs 16 kHz input, therefore:

* 16 kHz (recommended for a voice satellite): resample TTS audio to 16 kHz on the
  gateway/server; zero CPU cost on the device.
* 32 / 48 kHz: `Mic::read` low-pass filters and decimates to 16 kHz on the device
  (`Mic::sample_rate()` returns 16000, `Mic::read_raw` gives the native rate), and
  the speaker plays the native rate. Use it when playback quality matters.
  Verified with `KORVO2_SAMPLE_RATE=48000 cargo run --release --example loopback`:
  real time, 16 kHz mic frames, same tone level on the reference as at 16 kHz.
* 22.05 / 24 kHz: playback only; `Mic::read` returns native-rate frames that the
  AFE cannot use.

## ESP-SR models (WakeNet, VADNet)

Models are chosen in `sdkconfig.defaults` and packed by the ESP-SR component into
`srmodels.bin` during the build:

```ini
CONFIG_SR_WN_WN9_JARVIS_TTS=y     # "Jarvis"  (wn9_jarvis_tts)
CONFIG_SR_WN_WN9_HIESP=y          # "Hi ESP"  (wn9_hiesp)
CONFIG_SR_VADN_VADNET1_MEDIUM=y   # neural VAD (vadnet1_medium)
```

The list of available wake words is in ESP-SR's
[`wakeword_list.md`](https://github.com/espressif/esp-sr/blob/master/wakeword_list.md)
and `Kconfig.projbuild` (`CONFIG_SR_WN_*`). Several WakeNet models can be enabled;
the AFE runs up to two at once (`AfeConfig::wakenet2`).

`srmodels.bin` must be written to the `model` partition (offset **0x610000** in
`partitions.csv`). It is produced in the esp-idf-sys build directory:

```sh
SRMODELS=$(ls target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*/out/build/srmodels/srmodels.bin)
espflash write-bin --chip esp32s3 0x610000 "$SRMODELS"
```

It only needs to be re-flashed when the model selection changes. The release
images attached to GitHub Releases already contain it (merged image).

## Using the crates in your own firmware

```toml
[dependencies]
esp-idf-svc = "0.53"
esp-afe = { git = "https://github.com/jmorille/esp32-s3-korvo2-rs" }
esp32-s3-korvo2 = { git = "https://github.com/jmorille/esp32-s3-korvo2-rs" }
```

* Both crates must be **direct** dependencies of your binary crate: esp-idf-sys only
  collects `extra_components` (ESP-SR, esp_codec_dev) from the root crate and its
  direct dependencies.
* Copy `partitions.csv` and the relevant parts of `sdkconfig.defaults` (octal PSRAM,
  16 MB flash, custom partition table, models, and the
  `CONFIG_*_SKIP_LEGACY_CONFLICT_CHECK=y` lines: the BSP uses the new I2C / I2S / ADC
  drivers while `esp-idf-hal` links the legacy ones).
* `CONFIG_PARTITION_TABLE_CUSTOM_FILENAME` is resolved relative to the CMake project
  that esp-idf-sys generates in `target/<triple>/<profile>/build/esp-idf-sys-*/out`,
  hence `"../../../../../../partitions.csv"`.
* ESP-SR is CPU hungry: run the AFE fetch loop on core 1 with a priority above the
  main task (1), keep feeding from a high-priority thread, and avoid heavy work at
  higher priority on the same core, otherwise the AFE ring buffer overflows
  (see `examples/common/mod.rs::spawn_pinned`).

## Network demo: `wifi_stream`

Credentials are never committed: they come from `WIFI_SSID` / `WIFI_PASS` /
`STREAM_URL` at **build time** (`option_env!`), or from NVS namespace `wifi_stream`
(keys `ssid`, `pass`, `url`), which wins. The device looks up `_jarvis._tcp` with
mDNS (TXT `path` optional) and falls back to `STREAM_URL`.

Test server without any gateway (Python ≥ 3.9, stdlib only; advertises mDNS when
the optional `zeroconf` package is installed):

```sh
python3 tools/ws_echo_server.py --port 8765          # echoes PCM and pings back
```

## CI and releases

* `ci.yml`: rustfmt, clippy `-D warnings`, release build of all examples for
  `xtensa-esp32s3-espidf`, host unit tests of `esp-afe`, `cargo doc`. Cached:
  `~/.cargo`, `target/`, `.embuild/`.
* `release.yml`: [release-plz](https://release-plz.dev) opens a release PR
  (version bump + `CHANGELOG.md`); merging it creates the tags and GitHub Releases.
  Publishing to crates.io only happens if the `CARGO_REGISTRY_TOKEN` repository
  secret is set. On each release tag, the examples are built and attached as merged,
  flashable 16 MB images (`espflash save-image --merge`, models included):
  `espflash write-bin 0x0 loopback-merged.bin`.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option. Unless you explicitly state otherwise,
any contribution intentionally submitted for inclusion in this work shall be dual
licensed as above, without any additional terms or conditions.

Portions are adapted from the MIT-licensed
[xiaozhi-esp32](https://github.com/78/xiaozhi-esp32) board code (pin map, codec
I2S bring-up), see [NOTICE](NOTICE). ESP-IDF, ESP-SR and esp_codec_dev are
downloaded at build time and keep their own licenses; ESP-SR models and libraries
are distributed under the Espressif license.

## En bref (FR)

Bibliothèque Rust pour la carte ESP32-S3-Korvo-2 : micros + référence AEC déjà
ordonnés pour l'AFE ESP-SR (`"MMR"`), haut-parleur, boutons, LEDs, batterie, et un
wrapper sûr d'ESP-SR (AFE, VAD, WakeNet). Brancher les deux câbles USB, interrupteur
Power sur ON, flasher les modèles (`srmodels.bin` à 0x610000) puis
`cargo run --release --example afe_vad`.
