# esp-afe

Safe Rust wrapper for the [ESP-SR](https://github.com/espressif/esp-sr) speech
front-end on ESP32-S3 (`std`, ESP-IDF via `esp-idf-sys`):

* `Afe` — Audio Front-End: AEC, BSS, NS, VAD / VADNet, AGC, WakeNet. Config
  builder from the input layout (`"MMR"` = 2 mics + playback reference),
  `split()` into `Send` feeder / fetcher halves.
* `wakenet::WakeNet` — standalone wake word detector.
* `Models` — models of the ESP-SR `model` partition.

ESP-SR is pulled from the Espressif component registry through
`package.metadata.esp-idf-sys`; this crate must be a direct dependency of your
binary. See the [repository README](https://github.com/jmorille/esp32-s3-korvo2-rs)
for model flashing and examples.

Licensed under MIT OR Apache-2.0.
