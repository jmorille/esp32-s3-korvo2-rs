// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microphone and speaker handles.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::dsp::Decimator;
use crate::{Error, Shared};

/// Microphone input: MIC1, MIC2 and the AEC reference (ES7210 MIC3).
///
/// The ES7210 sends 4 TDM slots in the order `MIC1, MIC3, MIC2, MIC4`;
/// [`Mic::read`] reorders them to `[MIC1, MIC2, REF]` per frame.
///
/// When the board runs the shared I2S clock at 32 or 48 kHz
/// ([`crate::Config::sample_rate`]), [`Mic::read`] low-pass filters and decimates
/// to **16 kHz** so the frames stay usable by the ESP-SR AFE; [`Mic::read_raw`]
/// always returns the native I2S rate.
pub struct Mic {
    shared: Arc<Shared>,
    raw: Vec<i16>,
    ordered: Vec<i16>,
    i2s_rate: u32,
    decimator: Option<Decimator>,
}

impl Mic {
    /// Channel layout produced by [`Mic::read`], for `esp_afe::AfeConfig::new`.
    pub const AFE_FORMAT: &'static str = "MMR";
    /// Channels per frame produced by [`Mic::read`].
    pub const CHANNELS: usize = 3;
    /// Channels per frame produced by [`Mic::read_raw`] (TDM slots).
    pub const RAW_CHANNELS: usize = 4;

    pub(crate) fn new(shared: Arc<Shared>, i2s_rate: u32) -> Self {
        let decimator = match i2s_rate {
            32_000 => Some(Decimator::new(2, Self::CHANNELS)),
            48_000 => Some(Decimator::new(3, Self::CHANNELS)),
            _ => None,
        };
        Self {
            shared,
            raw: Vec::new(),
            ordered: Vec::new(),
            i2s_rate,
            decimator,
        }
    }

    /// Sample rate of the frames returned by [`Mic::read`] (16000 when decimating).
    pub fn sample_rate(&self) -> u32 {
        if self.decimator.is_some() {
            16_000
        } else {
            self.i2s_rate
        }
    }

    /// Native I2S sample rate (frames returned by [`Mic::read_raw`]).
    pub fn i2s_sample_rate(&self) -> u32 {
        self.i2s_rate
    }

    /// Fill `buf` with interleaved `[MIC1, MIC2, REF]` frames at
    /// [`Mic::sample_rate`] (blocking). `buf.len()` must be a multiple of 3.
    pub fn read(&mut self, buf: &mut [i16]) -> Result<(), Error> {
        if buf.len() % Self::CHANNELS != 0 {
            return Err(Error::BadLength {
                multiple_of: Self::CHANNELS,
                got: buf.len(),
            });
        }
        let factor = match self.i2s_rate {
            32_000 if self.decimator.is_some() => 2,
            48_000 if self.decimator.is_some() => 3,
            _ => 1,
        };
        let frames = buf.len() / Self::CHANNELS * factor;
        let mut raw = core::mem::take(&mut self.raw);
        raw.resize(frames * Self::RAW_CHANNELS, 0);
        let r = self.shared.audio.read(&mut raw);
        if r.is_ok() {
            let reorder = |o: &mut [i16], i: &[i16]| {
                o[0] = i[0]; // MIC1
                o[1] = i[2]; // MIC2
                o[2] = i[1]; // MIC3 = reference
            };
            match &mut self.decimator {
                None => {
                    for (o, i) in buf
                        .chunks_exact_mut(Self::CHANNELS)
                        .zip(raw.chunks_exact(Self::RAW_CHANNELS))
                    {
                        reorder(o, i);
                    }
                }
                Some(d) => {
                    self.ordered.resize(frames * Self::CHANNELS, 0);
                    for (o, i) in self
                        .ordered
                        .chunks_exact_mut(Self::CHANNELS)
                        .zip(raw.chunks_exact(Self::RAW_CHANNELS))
                    {
                        reorder(o, i);
                    }
                    d.process(&self.ordered, buf);
                }
            }
        }
        self.raw = raw;
        r
    }

    /// Fill `buf` with the raw 4-slot TDM frames `[MIC1, MIC3/REF, MIC2, MIC4]` at
    /// the native I2S rate.
    pub fn read_raw(&mut self, buf: &mut [i16]) -> Result<(), Error> {
        if buf.len() % Self::RAW_CHANNELS != 0 {
            return Err(Error::BadLength {
                multiple_of: Self::RAW_CHANNELS,
                got: buf.len(),
            });
        }
        self.shared.audio.read(buf)
    }

    /// Set the microphone gain in dB (0 ..= 37.5).
    pub fn set_gain(&mut self, db: f32) -> Result<(), Error> {
        self.shared.audio.set_in_gain(0b0011, db)
    }

    /// Set the AEC reference channel gain in dB (0 ..= 37.5).
    pub fn set_reference_gain(&mut self, db: f32) -> Result<(), Error> {
        self.shared.audio.set_in_gain(0b0100, db)
    }

    /// Mute / unmute the ADC.
    pub fn set_mute(&mut self, mute: bool) -> Result<(), Error> {
        self.shared.audio.set_in_mute(mute)
    }
}

/// Speaker output (ES8311 + NS4150), 16-bit mono PCM at the board sample rate.
///
/// Writes are split into 20 ms chunks so that [`Speaker::clear`] /
/// [`SpeakerControl::clear`] (barge-in) takes effect within one chunk, even while
/// another thread is blocked in [`Speaker::write`].
pub struct Speaker {
    shared: Arc<Shared>,
    sample_rate: u32,
}

impl Speaker {
    pub(crate) fn new(shared: Arc<Shared>, sample_rate: u32) -> Self {
        Self {
            shared,
            sample_rate,
        }
    }

    /// Sample rate in Hz (shared with the microphones' I2S clock, see
    /// [`crate::Config::sample_rate`]).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Play mono samples (blocking until queued to DMA). If [`Speaker::clear`]
    /// is called from another thread meanwhile, the rest of `pcm` is dropped.
    pub fn write(&mut self, pcm: &[i16]) -> Result<(), Error> {
        let generation = self.shared.clear_gen.load(Ordering::Acquire);
        let chunk = (self.sample_rate / 50).max(1) as usize; // 20 ms
        for part in pcm.chunks(chunk) {
            let _guard = self
                .shared
                .tx_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if self.shared.clear_gen.load(Ordering::Acquire) != generation {
                return Ok(());
            }
            self.shared.audio.write(part)?;
        }
        Ok(())
    }

    /// Drop all audio queued in the I2S DMA buffers now (barge-in). See
    /// [`SpeakerControl::clear`].
    pub fn clear(&mut self) -> Result<(), Error> {
        self.control().clear()
    }

    /// A cloneable handle to stop / mute / change the volume from other threads.
    pub fn control(&self) -> SpeakerControl {
        SpeakerControl {
            shared: self.shared.clone(),
        }
    }

    /// Set the volume, 0 ..= 100.
    pub fn set_volume(&mut self, volume: u8) -> Result<(), Error> {
        self.control().set_volume(volume)
    }

    /// Current volume.
    pub fn volume(&self) -> u8 {
        self.shared.volume.load(Ordering::Relaxed)
    }

    /// Change the volume by `delta`, saturating to 0 ..= 100. Returns the new volume.
    pub fn change_volume(&mut self, delta: i16) -> Result<u8, Error> {
        self.control().change_volume(delta)
    }

    /// Mute / unmute the DAC.
    pub fn set_mute(&mut self, mute: bool) -> Result<(), Error> {
        self.control().set_mute(mute)
    }

    /// Whether the output is muted.
    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }
}

/// Thread-safe control handle of the [`Speaker`] (`Clone + Send + Sync`).
#[derive(Clone)]
pub struct SpeakerControl {
    shared: Arc<Shared>,
}

impl SpeakerControl {
    /// Drop the audio queued in the I2S TX DMA buffers immediately (barge-in).
    ///
    /// The TX channel is disabled, its DMA buffers are pre-loaded with silence and
    /// it is re-enabled; a [`Speaker::write`] in progress on another thread stops
    /// after its current 20 ms chunk. The microphones keep running (RX is not
    /// touched).
    ///
    /// Measured on a V3.1 board with the `barge_in` example (16 kHz, 20 ms
    /// writes; tone observed on the AEC reference channel): the output is silent
    /// **~27 ms** (max 28 ms) after `clear()` versus **~119 ms** (max 120 ms) when
    /// the writes merely stop. Both figures include up to ~15 ms of I2S RX DMA
    /// buffering on the measuring side, so the real residual after `clear()` is
    /// about 10–25 ms.
    pub fn clear(&self) -> Result<(), Error> {
        self.shared.clear_gen.fetch_add(1, Ordering::AcqRel);
        let _guard = self
            .shared
            .tx_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.shared.audio.clear_tx()
    }

    /// Set the volume, 0 ..= 100.
    pub fn set_volume(&self, volume: u8) -> Result<(), Error> {
        let v = volume.min(100);
        self.shared.audio.set_out_volume(i32::from(v))?;
        self.shared.volume.store(v, Ordering::Relaxed);
        Ok(())
    }

    /// Current volume.
    pub fn volume(&self) -> u8 {
        self.shared.volume.load(Ordering::Relaxed)
    }

    /// Change the volume by `delta`, saturating to 0 ..= 100. Returns the new volume.
    pub fn change_volume(&self, delta: i16) -> Result<u8, Error> {
        let v = (i16::from(self.volume()) + delta).clamp(0, 100) as u8;
        self.set_volume(v)?;
        Ok(v)
    }

    /// Mute / unmute the DAC.
    pub fn set_mute(&self, mute: bool) -> Result<(), Error> {
        self.shared.audio.set_out_mute(mute)?;
        self.shared.muted.store(mute, Ordering::Relaxed);
        Ok(())
    }

    /// Whether the output is muted.
    pub fn is_muted(&self) -> bool {
        self.shared.muted.load(Ordering::Relaxed)
    }
}
