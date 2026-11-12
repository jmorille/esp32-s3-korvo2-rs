# esp32-s3-korvo2

Board support for the Espressif **ESP32-S3-Korvo-2 V3.x** (`std`, ESP-IDF):
ES8311 speaker path and ES7210 microphones + AEC reference over I2S0 full duplex
(via `esp_codec_dev`), frames ordered for the ESP-SR AFE (`"MMR"`), volume / mute /
gains, debounced ADC buttons, LEDs on the I/O expander and battery voltage.

```rust,ignore
let board = esp32_s3_korvo2::Korvo2::take()?;
let (mut mic, mut speaker) = (board.mic, board.speaker);
```

Examples: `loopback`, `afe_vad`, `wakeword`, `afe_wakeword`. See the
[repository README](https://github.com/jmorille/esp32-s3-korvo2-rs).

Licensed under MIT OR Apache-2.0.
