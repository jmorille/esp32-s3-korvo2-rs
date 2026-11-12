// SPDX-License-Identifier: MIT OR Apache-2.0
//! The AFE instance and its feeder / fetcher halves.

use std::sync::Arc;
use std::time::Duration;

use crate::raw::{self, Feature};
use crate::{AfeConfig, Error, Models, WakeWord};

/// VAD decision for a fetched frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vad {
    /// Noise or silence.
    Silence,
    /// Speech.
    Speech,
}

/// One processed output frame.
#[derive(Debug, Clone, PartialEq)]
pub struct AfeFrame {
    /// Processed mono audio (`fetch_chunk_size()` samples, 16 kHz).
    pub data: Vec<i16>,
    /// Audio preceding the speech onset that VAD held back, to prepend to `data`
    /// when `vad` turns to [`Vad::Speech`] (often empty).
    pub vad_cache: Vec<i16>,
    /// VAD state.
    pub vad: Vad,
    /// Set when a wake word was detected in this frame.
    pub wake_word: Option<WakeWord>,
    /// Input energy in dBFS (before AGC).
    pub volume_db: f32,
    /// Output channel chosen by the AFE.
    pub trigger_channel: i32,
    /// Free ratio of the internal ring buffer (low = fetch is falling behind).
    pub ringbuf_free: f32,
}

/// Static facts about an AFE instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AfeInfo {
    /// Samples **per channel** expected by each `feed()`.
    pub feed_chunk_size: usize,
    /// Samples returned by each `fetch()`.
    pub fetch_chunk_size: usize,
    /// Number of interleaved input channels.
    pub channel_num: usize,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl AfeInfo {
    /// Total `i16` values to pass to `feed()` (all channels interleaved).
    pub fn feed_len(&self) -> usize {
        self.feed_chunk_size * self.channel_num
    }
}

/// Wake word names of the configured WakeNet models (outer index = model - 1).
type WordTable = Arc<Vec<Vec<String>>>;

fn frame(r: raw::RawFetch, words: &WordTable) -> AfeFrame {
    AfeFrame {
        data: r.data,
        vad_cache: r.vad_cache,
        vad: if r.speech { Vad::Speech } else { Vad::Silence },
        wake_word: r.wake.map(|(w, m, l)| {
            let (w, m) = (w.max(0) as usize, m.max(0) as usize);
            let name = m
                .checked_sub(1)
                .and_then(|mi| words.get(mi))
                .and_then(|ws| w.checked_sub(1).and_then(|wi| ws.get(wi)))
                .cloned()
                .unwrap_or_default();
            WakeWord {
                index: w,
                name,
                model_index: m,
                length_samples: l.max(0) as usize,
            }
        }),
        volume_db: r.volume_db,
        trigger_channel: r.trigger_channel,
        ringbuf_free: r.ringbuf_free_pct,
    }
}

/// An ESP-SR audio front-end instance.
///
/// Use it directly from one thread, or [`Afe::split`] it into a feeder (audio
/// input thread) and a fetcher (processing thread), which is how ESP-SR is meant
/// to be driven. The AFE is destroyed when the last half is dropped.
pub struct Afe {
    raw: Arc<raw::RawAfe>,
    info: AfeInfo,
    words: WordTable,
}

impl Afe {
    /// Create an AFE. `models` is required for WakeNet / VADNet / NSNet.
    ///
    /// Call it from the main task: with ESP-SR 2.6.2 on the ESP32-S3, creating
    /// the AFE from a spawned `std::thread` was observed to crash inside
    /// `afe_create_from_config`. The feeder / fetcher halves can then be moved
    /// to other threads.
    pub fn new(config: &AfeConfig, models: Option<&Models>) -> Result<Self, Error> {
        let raw = raw::RawAfe::new(config, models.map(|m| m.inner.clone()))?;
        let info = AfeInfo {
            feed_chunk_size: raw.feed_chunk_size()?,
            fetch_chunk_size: raw.fetch_chunk_size()?,
            channel_num: raw.channel_num()?,
            sample_rate: raw.sample_rate()?,
        };
        let words = config
            .wakenet_models()
            .iter()
            .map(|m| models.map(|ms| ms.wake_words(m)).unwrap_or_default())
            .collect();
        Ok(Self {
            raw: Arc::new(raw),
            info,
            words: Arc::new(words),
        })
    }

    /// Chunk sizes, channel count and sample rate.
    pub fn info(&self) -> AfeInfo {
        self.info
    }

    /// Samples per channel expected by each `feed()`.
    pub fn feed_chunk_size(&self) -> usize {
        self.info.feed_chunk_size
    }

    /// Samples returned by each `fetch()`.
    pub fn fetch_chunk_size(&self) -> usize {
        self.info.fetch_chunk_size
    }

    /// Number of interleaved input channels.
    pub fn channel_num(&self) -> usize {
        self.info.channel_num
    }

    /// Feed one interleaved chunk of `feed_chunk_size() * channel_num()` samples.
    pub fn feed(&mut self, samples: &[i16]) -> Result<(), Error> {
        feed(&self.raw, &self.info, samples)
    }

    /// Fetch one processed frame (waits up to 2 s).
    pub fn fetch(&mut self) -> Result<AfeFrame, Error> {
        self.fetch_timeout(Duration::from_secs(2))
    }

    /// Fetch one processed frame, waiting at most `timeout`.
    pub fn fetch_timeout(&mut self, timeout: Duration) -> Result<AfeFrame, Error> {
        Ok(frame(self.raw.fetch(ticks(timeout))?, &self.words))
    }

    /// Enable or disable a processing stage at runtime.
    pub fn set_enabled(&mut self, feature: Feature, enable: bool) -> Result<(), Error> {
        self.raw.toggle(feature, enable)
    }

    /// Resume wake word detection.
    pub fn enable_wakenet(&mut self) -> Result<(), Error> {
        self.raw.toggle(Feature::WakeNet, true)
    }

    /// Pause wake word detection (e.g. while a conversation is in progress).
    pub fn disable_wakenet(&mut self) -> Result<(), Error> {
        self.raw.toggle(Feature::WakeNet, false)
    }

    /// Log the processing pipeline (e.g. `[input] -> |AEC| -> |WakeNet| -> [output]`).
    pub fn print_pipeline(&self) -> Result<(), Error> {
        self.raw.print_pipeline()
    }

    /// Split into a [`AfeFeeder`] and a [`AfeFetcher`] that can live on two threads.
    pub fn split(self) -> (AfeFeeder, AfeFetcher) {
        (
            AfeFeeder {
                raw: self.raw.clone(),
                info: self.info,
            },
            AfeFetcher {
                raw: self.raw,
                info: self.info,
                words: self.words,
            },
        )
    }
}

/// Input half of a split [`Afe`].
pub struct AfeFeeder {
    raw: Arc<raw::RawAfe>,
    info: AfeInfo,
}

impl AfeFeeder {
    /// Chunk sizes, channel count and sample rate.
    pub fn info(&self) -> AfeInfo {
        self.info
    }

    /// Number of `i16` values expected by [`AfeFeeder::feed`].
    pub fn chunk_len(&self) -> usize {
        self.info.feed_len()
    }

    /// Feed one interleaved chunk (see [`Afe::feed`]).
    pub fn feed(&mut self, samples: &[i16]) -> Result<(), Error> {
        feed(&self.raw, &self.info, samples)
    }
}

/// Output / control half of a split [`Afe`].
pub struct AfeFetcher {
    raw: Arc<raw::RawAfe>,
    info: AfeInfo,
    words: WordTable,
}

impl AfeFetcher {
    /// Chunk sizes, channel count and sample rate.
    pub fn info(&self) -> AfeInfo {
        self.info
    }

    /// Fetch one processed frame (waits up to 2 s).
    pub fn fetch(&mut self) -> Result<AfeFrame, Error> {
        self.fetch_timeout(Duration::from_secs(2))
    }

    /// Fetch one processed frame, waiting at most `timeout`.
    pub fn fetch_timeout(&mut self, timeout: Duration) -> Result<AfeFrame, Error> {
        Ok(frame(self.raw.fetch(ticks(timeout))?, &self.words))
    }

    /// Enable or disable a processing stage at runtime.
    pub fn set_enabled(&mut self, feature: Feature, enable: bool) -> Result<(), Error> {
        self.raw.toggle(feature, enable)
    }

    /// Resume wake word detection.
    pub fn enable_wakenet(&mut self) -> Result<(), Error> {
        self.raw.toggle(Feature::WakeNet, true)
    }

    /// Pause wake word detection.
    pub fn disable_wakenet(&mut self) -> Result<(), Error> {
        self.raw.toggle(Feature::WakeNet, false)
    }

    /// Reset the VAD state machine.
    pub fn reset_vad(&mut self) -> Result<(), Error> {
        self.raw.reset_vad()
    }

    /// Drop buffered audio.
    pub fn reset_buffer(&mut self) -> Result<(), Error> {
        self.raw.reset_buffer()
    }

    /// Set the detection threshold (0.4 ..= 0.9999) of WakeNet model 1 or 2.
    pub fn set_wakenet_threshold(&mut self, model: i32, threshold: f32) -> Result<(), Error> {
        self.raw.set_wakenet_threshold(model, threshold)
    }

    /// Log the processing pipeline.
    pub fn print_pipeline(&self) -> Result<(), Error> {
        self.raw.print_pipeline()
    }
}

fn feed(raw: &raw::RawAfe, info: &AfeInfo, samples: &[i16]) -> Result<(), Error> {
    let expected = info.feed_len();
    if samples.len() != expected {
        return Err(Error::BadChunk {
            expected,
            got: samples.len(),
        });
    }
    raw.feed(samples)
}

fn ticks(d: Duration) -> u32 {
    let hz = u64::from(esp_idf_sys::configTICK_RATE_HZ);
    let t = (d.as_millis() as u64 * hz).div_ceil(1000);
    t.min(u64::from(u32::MAX)) as u32
}
