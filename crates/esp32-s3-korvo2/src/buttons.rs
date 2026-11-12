// SPDX-License-Identifier: MIT OR Apache-2.0
//! The six front panel buttons on the GPIO5 resistor ladder.

use crate::{ffi, Error};

/// A front panel button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    /// REC (≈ 2.41 V).
    Rec,
    /// MUTE (≈ 1.98 V).
    Mute,
    /// PLAY (≈ 1.65 V).
    Play,
    /// SET (≈ 1.11 V).
    Set,
    /// VOL- (≈ 0.82 V).
    VolDown,
    /// VOL+ (≈ 0.38 V).
    VolUp,
}

impl Button {
    /// Nominal ladder voltage from the schematic, in mV.
    pub const fn millivolts(self) -> i32 {
        match self {
            Self::Rec => 2410,
            Self::Mute => 1980,
            Self::Play => 1650,
            Self::Set => 1110,
            Self::VolDown => 820,
            Self::VolUp => 380,
        }
    }

    /// All buttons.
    pub const ALL: [Button; 6] = [
        Self::Rec,
        Self::Mute,
        Self::Play,
        Self::Set,
        Self::VolDown,
        Self::VolUp,
    ];

    /// Button whose nominal voltage is within ±150 mV of `mv` (none pressed ≈ 3.3 V).
    pub fn from_millivolts(mv: i32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|b| (b.millivolts() - mv).abs() <= 150)
    }
}

/// Edge produced by [`Buttons::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonEvent {
    /// The button was pressed.
    Pressed(Button),
    /// The button was released.
    Released(Button),
}

/// Button reader (ADC1 channel 4, one-shot, calibrated).
pub struct Buttons {
    adc: ffi::AdcChannel,
    stable: Option<Button>,
    candidate: Option<Button>,
}

impl Buttons {
    pub(crate) fn new(adc: &std::sync::Arc<ffi::Adc1>) -> Result<Self, Error> {
        Ok(Self {
            adc: ffi::AdcChannel::new(adc, ffi::ADC_CH_BUTTONS)?,
            stable: None,
            candidate: None,
        })
    }

    /// Voltage on the ladder, in mV.
    pub fn read_millivolts(&mut self) -> Result<i32, Error> {
        self.adc.read_mv()
    }

    /// Button currently held down, if any (no debouncing).
    pub fn pressed(&mut self) -> Result<Option<Button>, Error> {
        Ok(Button::from_millivolts(self.read_millivolts()?))
    }

    /// Sample once and return a press / release edge, debounced over two
    /// consecutive identical samples. Call every 10–20 ms.
    pub fn poll(&mut self) -> Result<Option<ButtonEvent>, Error> {
        let now = self.pressed()?;
        if now != self.candidate {
            self.candidate = now;
            return Ok(None);
        }
        if now == self.stable {
            return Ok(None);
        }
        let prev = core::mem::replace(&mut self.stable, now);
        Ok(match (prev, now) {
            (_, Some(b)) => Some(ButtonEvent::Pressed(b)),
            (Some(b), None) => Some(ButtonEvent::Released(b)),
            (None, None) => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Button;

    #[test]
    fn ladder() {
        assert_eq!(Button::from_millivolts(2400), Some(Button::Rec));
        assert_eq!(Button::from_millivolts(400), Some(Button::VolUp));
        assert_eq!(Button::from_millivolts(3200), None);
    }
}
