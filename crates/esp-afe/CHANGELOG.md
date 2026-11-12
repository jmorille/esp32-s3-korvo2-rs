# Changelog

All notable changes to this crate are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/jmorille/esp32-s3-korvo2-rs/compare/esp-afe-v0.1.0...esp-afe-v0.1.1) - 2026-11-12

### Added

- *(korvo2)* speaker clear for barge-in, 32/48 kHz support, AFE benchmark

### Other

- fix release-plz dirty tree, espflash PATH; add changelogs

### Documentation

- `Afe::new`: create the AFE from the main task (ESP-SR 2.6.2 crashes when it
  is created from a spawned thread).

## [0.1.0] - 2026-11-12

### Added

- Initial release.
