// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microphone and speaker handles.

use std::sync::Arc;

use crate::{Error, Shared};

/// Microphone input: MIC1, MIC2 and the AEC reference (ES7210 MIC3).
///
/// The ES7210 sends 4 TDM slots in the order `MIC1, MIC3, MIC2, MIC4`;
/// [`Mic::read`] reorders them to `[MIC1, MIC2, REF]` per frame.
pub struct Mic {
    shared: Arc<Shared>,
    raw: Vec<i16>,
    sample_rate: u32,
}

impl Mic {
    /// Channel layout produced by [`Mic::read`], for `esp_afe::AfeConfig::new`.
    pub const AFE_FORMAT: &'static str = "MMR";
    /// Channels per frame produced by [`Mic::read`].
    pub const CHANNELS: usize = 3;
    /// Channels per frame produced by [`Mic::read_raw`] (TDM slots).
    pub const RAW_CHANNELS: usize = 4;

    pub(crate) fn new(shared: Arc<Shared>, sample_rate: u32) -> Self {
        Self {
            shared,
            raw: Vec::new(),
            sample_rate,
        }
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Fill `buf` with interleaved `[MIC1, MIC2, REF]` frames (blocking).
    /// `buf.len()` must be a multiple of 3.
    pub fn read(&mut self, buf: &mut [i16]) -> Result<(), Error> {
        if buf.len() % Self::CHANNELS != 0 {
            return Err(Error::BadLength {
                multiple_of: Self::CHANNELS,
                got: buf.len(),
            });
        }
        let frames = buf.len() / Self::CHANNELS;
        let mut raw = core::mem::take(&mut self.raw);
        raw.resize(frames * Self::RAW_CHANNELS, 0);
        let r = self.shared.audio.read(&mut raw);
        if r.is_ok() {
            for (o, i) in buf
                .chunks_exact_mut(Self::CHANNELS)
                .zip(raw.chunks_exact(Self::RAW_CHANNELS))
            {
                o[0] = i[0]; // MIC1
                o[1] = i[2]; // MIC2
                o[2] = i[1]; // MIC3 = reference
            }
        }
        self.raw = raw;
        r
    }

    /// Fill `buf` with the raw 4-slot TDM frames `[MIC1, MIC3/REF, MIC2, MIC4]`.
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

/// Speaker output (ES8311 + NS4150), 16-bit mono PCM.
pub struct Speaker {
    shared: Arc<Shared>,
    sample_rate: u32,
    volume: u8,
    muted: bool,
}

impl Speaker {
    pub(crate) fn new(shared: Arc<Shared>, sample_rate: u32, volume: u8) -> Self {
        Self {
            shared,
            sample_rate,
            volume,
            muted: false,
        }
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Play mono samples (blocking until queued to DMA).
    pub fn write(&mut self, pcm: &[i16]) -> Result<(), Error> {
        self.shared.audio.write(pcm)
    }

    /// Set the volume, 0 ..= 100.
    pub fn set_volume(&mut self, volume: u8) -> Result<(), Error> {
        let v = volume.min(100);
        self.shared.audio.set_out_volume(i32::from(v))?;
        self.volume = v;
        Ok(())
    }

    /// Current volume.
    pub fn volume(&self) -> u8 {
        self.volume
    }

    /// Change the volume by `delta`, saturating to 0 ..= 100. Returns the new volume.
    pub fn change_volume(&mut self, delta: i16) -> Result<u8, Error> {
        let v = (i16::from(self.volume) + delta).clamp(0, 100) as u8;
        self.set_volume(v)?;
        Ok(v)
    }

    /// Mute / unmute the DAC.
    pub fn set_mute(&mut self, mute: bool) -> Result<(), Error> {
        self.shared.audio.set_out_mute(mute)?;
        self.muted = mute;
        Ok(())
    }

    /// Whether the output is muted.
    pub fn is_muted(&self) -> bool {
        self.muted
    }
}
