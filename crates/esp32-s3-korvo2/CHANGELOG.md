# Changelog

All notable changes to this crate are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Speaker::clear()` / `SpeakerControl::clear()`: drop the queued I2S TX DMA audio
  for barge-in (measured ~27 ms residual vs ~119 ms without).
- `SpeakerControl`: cloneable, thread-safe stop / volume / mute handle.
- `Config::sample_rate` accepts 22050, 24000, 32000 and 48000 Hz (shared TX/RX
  clock); at 32 / 48 kHz `Mic::read` decimates to 16 kHz for the AFE.
- `Mic::i2s_sample_rate`, `Error::Unsupported`.
- Examples `barge_in` and `afe_bench`.

## [0.1.0] - 2026-11-12

### Added

- Initial release.
