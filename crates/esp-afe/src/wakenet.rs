// SPDX-License-Identifier: MIT OR Apache-2.0
//! Standalone WakeNet wake word detector (`esp_wn_iface.h` / `esp_wn_models.h`),
//! usable without the AFE pipeline, e.g. on one raw microphone channel.
//!
//! ```ignore
//! use esp_afe::wakenet::{DetMode, ModelList, WakeNet};
//!
//! let models = ModelList::load("model")?;
//! log::info!("wake word models: {:?}", models.wakenet_names());
//! let mut wn = WakeNet::new(&models, "wn9_hiesp", DetMode::Aggressive)?;
//! let mut chunk = vec![0i16; wn.chunk_len()];   // 16 kHz mono
//! if let Some(w) = wn.detect(&chunk)? {
//!     log::info!("detected {} (#{})", w.name, w.index);
//! }
//! ```
//!
//! Without the AFE there is no AEC or noise suppression: expect more misses
//! and false alarms when the speaker is playing or the room is noisy.

use crate::{raw, Error, WakeWord};

pub use crate::config::DetMode;
pub use crate::model_info::WAKENET_PREFIX;

/// The model partition (alias of [`crate::Models`]).
pub type ModelList = crate::Models;

/// A WakeNet model instance.
///
/// Created with `esp_wn_handle_from_name()` + `create()`, destroyed on drop.
/// Keeps the model list alive while it exists.
pub struct WakeNet {
    raw: raw::RawWakeNet,
    model: String,
    chunk: usize,
    channels: usize,
    words: Vec<String>,
}

impl core::fmt::Debug for WakeNet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WakeNet")
            .field("model", &self.model)
            .field("chunk", &self.chunk)
            .field("channels", &self.channels)
            .field("words", &self.words)
            .finish()
    }
}

impl WakeNet {
    /// Load WakeNet model `model_name` (e.g. `"wn9_jarvis_tts"`) from `models`.
    pub fn new(models: &ModelList, model_name: &str, mode: DetMode) -> Result<Self, Error> {
        let raw = raw::RawWakeNet::new(models.inner.clone(), model_name, mode.raw())?;
        let chunk = raw.chunk_size()?;
        let channels = raw.channel_num()?;
        let n = raw.word_count()?;
        let mut words = Vec::with_capacity(n);
        for i in 1..=n {
            words.push(raw.word_name(i)?.unwrap_or_default());
        }
        Ok(Self {
            raw,
            model: model_name.to_owned(),
            chunk,
            channels,
            words,
        })
    }

    /// Model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Samples per channel expected by each [`WakeNet::detect`] call.
    pub fn chunk_size(&self) -> usize {
        self.chunk
    }

    /// Interleaved channels expected by [`WakeNet::detect`] (1 for single-channel modes).
    pub fn channel_num(&self) -> usize {
        self.channels
    }

    /// Total `i16` values per [`WakeNet::detect`] call (`chunk_size * channel_num`).
    pub fn chunk_len(&self) -> usize {
        self.chunk * self.channels
    }

    /// Expected sample rate (16000 Hz).
    pub fn sample_rate(&self) -> Result<u32, Error> {
        self.raw.sample_rate()
    }

    /// Number of wake words in the model.
    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    /// Name of wake word `index` (starting at 1), e.g. `"Jarvis"`.
    pub fn word_name(&self, index: usize) -> Option<&str> {
        index
            .checked_sub(1)
            .and_then(|i| self.words.get(i))
            .map(String::as_str)
    }

    /// Detection threshold of word `index` (starting at 1).
    pub fn threshold(&self, index: usize) -> Result<f32, Error> {
        self.raw.threshold(index)
    }

    /// Set the detection threshold (0.4 ..= 0.9999) of word `index` (starting at 1).
    pub fn set_threshold(&mut self, index: usize, threshold: f32) -> Result<(), Error> {
        self.raw.set_threshold(index, threshold.clamp(0.4, 0.9999))
    }

    /// Restore the model's default thresholds.
    pub fn reset_threshold(&mut self) -> Result<(), Error> {
        self.raw.reset_threshold()
    }

    /// Run detection on one chunk of [`WakeNet::chunk_len`] samples.
    pub fn detect(&mut self, samples: &[i16]) -> Result<Option<WakeWord>, Error> {
        let expected = self.chunk_len();
        if samples.len() != expected {
            return Err(Error::BadChunk {
                expected,
                got: samples.len(),
            });
        }
        let r = self.raw.detect(samples)?;
        if r <= 0 {
            // 0 = WAKENET_NO_DETECT, -1 = WAKENET_CHANNEL_VERIFIED
            return Ok(None);
        }
        let index = r as usize;
        let length = self.raw.start_point().unwrap_or(0).max(0) as usize;
        Ok(Some(WakeWord {
            index,
            name: self.word_name(index).unwrap_or_default().to_owned(),
            model_index: 1,
            length_samples: length,
        }))
    }

    /// Channel that triggered the last detection (multi-channel modes).
    pub fn triggered_channel(&self) -> Result<i32, Error> {
        self.raw.triggered_channel()
    }

    /// Reset the internal state (e.g. after a detection, to avoid re-triggering).
    pub fn clean(&mut self) -> Result<(), Error> {
        self.raw.clean()
    }
}
