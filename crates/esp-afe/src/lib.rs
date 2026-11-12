// SPDX-License-Identifier: MIT OR Apache-2.0
//! Safe Rust wrapper for the [ESP-SR](https://github.com/espressif/esp-sr) speech
//! front-end on ESP32-S3, for `std` applications built on ESP-IDF
//! (`esp-idf-sys` / `esp-idf-svc`).
//!
//! Two entry points:
//!
//! * [`Afe`] — the **Audio Front-End**. It takes interleaved 16 kHz / 16-bit
//!   microphone (and playback reference) frames and produces one cleaned-up
//!   channel, together with voice activity (VAD) and wake word (WakeNet) events.
//!   It runs acoustic echo cancellation (AEC), speech enhancement (BSS), noise
//!   suppression (NS) and automatic gain control (AGC) as configured.
//! * [`wakenet::WakeNet`] — a **standalone wake word detector** on one raw
//!   channel, without the AFE pipeline.
//!
//! Both read their neural models (WakeNet, VADNet...) from an ESP-SR model
//! partition through [`Models`].
//!
//! # AFE example
//!
//! ```ignore
//! use esp_afe::{Afe, AfeConfig, AecConfig, AecMode, AecNlpLevel, Models, VadConfig};
//!
//! let models = Models::load("model")?;             // ESP-SR "model" flash partition
//! let wn = models.find("wn9_", Some("jarvis"));     // pick a WakeNet model
//! let mut cfg = AfeConfig::new("MMR")?                // 2 mics + playback reference
//!     .aec(AecConfig { mode: AecMode::SrLowCost, nlp_level: AecNlpLevel::Aggressive, filter_length: None })
//!     .ns(true)
//!     .vad(VadConfig::default());
//! if let Some(wn) = &wn { cfg = cfg.wakenet(wn); }
//! let afe = Afe::new(&cfg, Some(&models))?;
//! let (mut feeder, mut fetcher) = afe.split();
//! // thread 1: feeder.feed(&interleaved_frame)?;    // feeder.chunk_len() samples
//! // thread 2: let frame = fetcher.fetch()?;         // frame.vad, frame.wake_word, frame.data
//! ```
//!
//! # Standalone WakeNet example
//!
//! ```ignore
//! use esp_afe::wakenet::{DetMode, ModelList, WakeNet};
//!
//! let models = ModelList::load("model")?;
//! let mut wn = WakeNet::new(&models, "wn9_jarvis_tts", DetMode::Aggressive)?;
//! let mut chunk = vec![0i16; wn.chunk_len()];
//! loop {
//!     // fill `chunk` with 16 kHz mono samples...
//!     if let Some(w) = wn.detect(&chunk)? { log::info!("wake word {} ({})", w.name, w.index); }
//! }
//! ```
//!
//! # Building
//!
//! The ESP-SR component (and its bindings in `esp_idf_sys::esp_sr`) is pulled in
//! automatically through this crate's `package.metadata.esp-idf-sys` section; it
//! must be a **direct** dependency of your binary crate for that to happen.
//! Models are selected with the `CONFIG_SR_WN_*` / `CONFIG_SR_VADN_*` sdkconfig
//! options, packed by ESP-SR into `srmodels.bin` and flashed to the `model`
//! partition (see the repository README).
//!
//! On non-ESP-IDF targets only the pure-Rust parts ([`AfeConfig`], [`model_info`])
//! are compiled, so they can be unit-tested on the host.
//!
//! All `unsafe` code lives in the private `raw` module.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod config;
pub mod model_info;

#[cfg(target_os = "espidf")]
mod afe;
#[cfg(target_os = "espidf")]
mod models;
#[cfg(target_os = "espidf")]
#[allow(unsafe_code)]
mod raw;
#[cfg(target_os = "espidf")]
pub mod wakenet;

pub use config::{
    AecConfig, AecMode, AecNlpLevel, AfeConfig, AfeMode, AfeType, AgcConfig, AgcMode, DetMode,
    MemoryAlloc, Toggle, VadConfig, VadMode,
};

#[cfg(target_os = "espidf")]
pub use afe::{Afe, AfeFeeder, AfeFetcher, AfeFrame, AfeInfo, Vad};
#[cfg(target_os = "espidf")]
pub use models::Models;
#[cfg(target_os = "espidf")]
pub use raw::Feature;

/// Errors returned by this crate.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// Input format string must only contain `M`, `R`, `N` and at least one `M`.
    InvalidInputFormat(String),
    /// A string contained an interior NUL byte.
    InvalidName,
    /// No model partition with this label (or it is empty / not formatted).
    ModelsNotFound(String),
    /// The requested model is not present in the model partition.
    ModelNotFound(String),
    /// `afe_config_init()` rejected the configuration.
    Config,
    /// The AFE / WakeNet instance could not be created (out of memory, bad config...).
    Create,
    /// `feed()` / `detect()` received a slice of the wrong length.
    BadChunk {
        /// Expected number of `i16` samples (all channels interleaved).
        expected: usize,
        /// Received number of samples.
        got: usize,
    },
    /// `feed()` returned an error code.
    Feed(i32),
    /// `fetch()` timed out (no frame available).
    Timeout,
    /// The interface table does not implement this operation.
    Unsupported(&'static str),
    /// A control operation failed.
    Op(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidInputFormat(s) => write!(f, "invalid AFE input format {s:?}"),
            Self::InvalidName => write!(f, "string contains a NUL byte"),
            Self::ModelsNotFound(p) => write!(f, "no ESP-SR models in partition {p:?}"),
            Self::ModelNotFound(m) => write!(f, "model {m:?} not found in model partition"),
            Self::Config => write!(f, "afe_config_init failed"),
            Self::Create => write!(f, "instance creation failed"),
            Self::BadChunk { expected, got } => {
                write!(f, "expected {expected} samples, got {got}")
            }
            Self::Feed(r) => write!(f, "AFE feed failed ({r})"),
            Self::Timeout => write!(f, "AFE fetch timed out"),
            Self::Unsupported(op) => write!(f, "operation {op} not supported"),
            Self::Op(op) => write!(f, "operation {op} failed"),
        }
    }
}

impl std::error::Error for Error {}

/// A wake word detection (from the AFE or from a standalone [`wakenet::WakeNet`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeWord {
    /// Index of the word inside its model, starting at 1.
    pub index: usize,
    /// Name of the word (e.g. `"Jarvis"`), empty if unknown.
    pub name: String,
    /// Which WakeNet model fired (1 or 2; always 1 for a standalone detector).
    pub model_index: usize,
    /// Length of the detected word in samples (0 if unknown).
    pub length_samples: usize,
}
