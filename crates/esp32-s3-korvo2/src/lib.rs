// SPDX-License-Identifier: MIT OR Apache-2.0
//! Board support for the Espressif **ESP32-S3-Korvo-2 V3.x** (V3.0 / V3.1), for
//! `std` applications on ESP-IDF.
//!
//! * **Audio in**: ES7210 4-channel ADC over I2S0 TDM. MIC1 / MIC2 are the two
//!   microphones, MIC3 carries the ES8311 DAC output as **AEC reference**.
//!   [`Mic`] returns interleaved frames already ordered `[MIC1, MIC2, REF]`, i.e.
//!   the `"MMR"` layout expected by the ESP-SR AFE ([`Mic::AFE_FORMAT`]).
//! * **Audio out**: ES8311 DAC over I2S0 (standard mode) + NS4150 amplifier
//!   (enabled on GPIO48). [`Speaker`] plays 16-bit mono PCM.
//! * **Buttons**: REC / MUTE / PLAY / SET / VOL- / VOL+ on the GPIO5 ADC ladder.
//! * **LEDs**: red and blue LEDs on the TCA9554 / TCA9554A I/O expander.
//! * **Battery**: voltage of the optional Li-ion cell on GPIO6 ([`Battery`]).
//!
//! Every handle is `Send`, so [`Mic`], [`Speaker`], [`Buttons`], [`Battery`] and
//! [`Leds`] (also `Clone`) can be moved to different threads.
//!
//! ```ignore
//! let board = esp32_s3_korvo2::Korvo2::take()?;
//! let (mut mic, mut speaker) = (board.mic, board.speaker);
//! let mut frame = vec![0i16; 3 * 256];          // 256 samples x [MIC1, MIC2, REF]
//! mic.read(&mut frame)?;
//! speaker.set_volume(60)?;
//! speaker.write(&[0i16; 256])?;
//! ```
//!
//! The `esp_codec_dev` component (bindings in `esp_idf_sys::codec`) is pulled in
//! through this crate's `package.metadata.esp-idf-sys`; this crate must be a
//! **direct** dependency of your binary crate.
//!
//! All `unsafe` code lives in the private `ffi` module.

#![deny(unsafe_code)]
#![warn(missing_docs)]

mod audio;
pub mod battery;
mod buttons;
#[allow(unsafe_code)]
mod ffi;
mod leds;
pub mod pins;

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use audio::{Mic, Speaker};
pub use battery::Battery;
pub use buttons::{Button, ButtonEvent, Buttons};
pub use leds::{Led, Leds};

/// Errors returned by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// [`Korvo2::take`] was already called.
    AlreadyTaken,
    /// An ESP-IDF call failed.
    Esp {
        /// `esp_err_t` code.
        code: i32,
        /// Function name.
        what: &'static str,
    },
    /// An `esp_codec_dev` call failed.
    Codec {
        /// Return code.
        code: i32,
        /// Function name.
        what: &'static str,
    },
    /// A driver object could not be created.
    Init(&'static str),
    /// A buffer has the wrong length.
    BadLength {
        /// Required multiple.
        multiple_of: usize,
        /// Received length.
        got: usize,
    },
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AlreadyTaken => write!(f, "Korvo2 board already taken"),
            Self::Esp { code, what } => write!(f, "{what} failed: esp_err {code:#x}"),
            Self::Codec { code, what } => write!(f, "{what} failed: {code}"),
            Self::Init(what) => write!(f, "{what} failed"),
            Self::BadLength { multiple_of, got } => {
                write!(f, "buffer length {got} is not a multiple of {multiple_of}")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Board configuration for [`Korvo2::take_with`].
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Sample rate of both directions, in Hz (ESP-SR needs 16000).
    pub sample_rate: u32,
    /// Initial speaker volume, 0 ..= 100.
    pub volume: u8,
    /// ES7210 gain of the two microphones, in dB (0 ..= 37.5).
    pub mic_gain_db: f32,
    /// ES7210 gain of the AEC reference channel (MIC3), in dB (0 ..= 37.5).
    pub reference_gain_db: f32,
    /// Supply voltage of the NS4150 amplifier, used by esp_codec_dev to scale the
    /// DAC gain (5.0 V when powered over USB).
    pub pa_voltage: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            volume: 60,
            mic_gain_db: 30.0,
            reference_gain_db: 24.0,
            pa_voltage: 5.0,
        }
    }
}

/// Hardware shared by the audio handles. Field order = drop order: the codec
/// devices (and their I2C control interfaces) are released before the bus.
pub(crate) struct Shared {
    pub(crate) audio: ffi::Audio,
    pub(crate) _bus: Arc<ffi::I2cBus>,
}

/// Expander handle shared by [`Leds`] clones.
pub(crate) type Expander = Arc<Mutex<Option<ffi::I2cDevice>>>;

/// The ESP32-S3-Korvo-2 board.
///
/// Owns I2C0 (GPIO17/18), I2S0 (GPIO16/9/45/8/10), GPIO48 (PA enable) and the
/// ADC1 one-shot unit (GPIO5 buttons, GPIO6 battery). Do not use these
/// peripherals elsewhere.
pub struct Korvo2 {
    /// Microphones + AEC reference.
    pub mic: Mic,
    /// Speaker output.
    pub speaker: Speaker,
    /// Front panel buttons.
    pub buttons: Buttons,
    /// LEDs on the I/O expander.
    pub leds: Leds,
    /// Battery voltage.
    pub battery: Battery,
}

static TAKEN: AtomicBool = AtomicBool::new(false);

impl Korvo2 {
    /// Initialise the board with [`Config::default`] (16 kHz, volume 60).
    pub fn take() -> Result<Self, Error> {
        Self::take_with(Config::default())
    }

    /// Initialise the board. Only one instance can exist.
    pub fn take_with(config: Config) -> Result<Self, Error> {
        if TAKEN.swap(true, Ordering::SeqCst) {
            return Err(Error::AlreadyTaken);
        }
        let r = Self::init(config);
        if r.is_err() {
            TAKEN.store(false, Ordering::SeqCst);
        }
        r
    }

    fn init(config: Config) -> Result<Self, Error> {
        let bus = Arc::new(ffi::I2cBus::new(0)?);
        let leds = Leds::new(&bus);

        let audio = ffi::Audio::new(
            &bus,
            &ffi::AudioParams {
                sample_rate: config.sample_rate,
                pa_voltage: config.pa_voltage,
            },
        )?;
        audio.open(config.sample_rate)?;
        // ES7210 gain masks use the physical MIC numbering (bit 0 = MIC1).
        audio.set_in_gain(0b0011, config.mic_gain_db)?;
        audio.set_in_gain(0b0100, config.reference_gain_db)?;
        audio.set_out_volume(i32::from(config.volume.min(100)))?;
        log::info!(
            "Korvo-2 audio ready: {} Hz, volume {}, mic gain {} dB, ref gain {} dB",
            config.sample_rate,
            config.volume,
            config.mic_gain_db,
            config.reference_gain_db
        );

        let adc = ffi::Adc1::new()?;
        let shared = Arc::new(Shared { audio, _bus: bus });
        Ok(Self {
            mic: Mic::new(shared.clone(), config.sample_rate),
            speaker: Speaker::new(shared, config.sample_rate, config.volume.min(100)),
            buttons: Buttons::new(&adc)?,
            battery: Battery::new(&adc)?,
            leds,
        })
    }
}
