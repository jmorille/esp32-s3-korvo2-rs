// SPDX-License-Identifier: MIT OR Apache-2.0
//! Battery voltage on GPIO6 (`BAT_MEAS_ADC`).
//!
//! Schematic V3.1.2, sheet 5: `VBAT` → R248 301 kΩ → `BAT_MEAS_ADC` → R401
//! 100 kΩ → GND, so `VBAT = V(GPIO6) × (301 + 100) / 100`.
//! Without a battery the pin reads the charger output (≈ 4.2 V on USB power).

use std::sync::Arc;

use crate::{ffi, Error};

/// Divider numerator (R248 + R401, kΩ).
pub const DIVIDER_TOTAL_KOHM: i32 = 401;
/// Divider denominator (R401, kΩ).
pub const DIVIDER_BOTTOM_KOHM: i32 = 100;

/// Battery voltage reader (ADC1 channel 5, one-shot, calibrated).
pub struct Battery {
    adc: ffi::AdcChannel,
}

impl Battery {
    pub(crate) fn new(adc: &Arc<ffi::Adc1>) -> Result<Self, Error> {
        Ok(Self {
            adc: ffi::AdcChannel::new(adc, ffi::ADC_CH_BATTERY)?,
        })
    }

    /// Voltage on GPIO6 (after the divider), in mV.
    pub fn read_pin_millivolts(&mut self) -> Result<i32, Error> {
        self.adc.read_mv()
    }

    /// Battery voltage in mV, averaged over 8 samples.
    pub fn read_millivolts(&mut self) -> Result<i32, Error> {
        let mut sum = 0;
        for _ in 0..8 {
            sum += self.adc.read_mv()?;
        }
        Ok(pin_to_battery_mv(sum / 8))
    }

    /// Rough state of charge of a 1-cell Li-ion battery, 0 ..= 100 %.
    pub fn percent(&mut self) -> Result<u8, Error> {
        Ok(lipo_percent(self.read_millivolts()?))
    }
}

/// Convert the voltage on GPIO6 to the battery voltage.
pub const fn pin_to_battery_mv(pin_mv: i32) -> i32 {
    pin_mv * DIVIDER_TOTAL_KOHM / DIVIDER_BOTTOM_KOHM
}

/// Rough Li-ion open-circuit voltage → state of charge (piecewise linear).
pub fn lipo_percent(mv: i32) -> u8 {
    const CURVE: [(i32, i32); 8] = [
        (3300, 0),
        (3500, 5),
        (3600, 10),
        (3700, 30),
        (3800, 50),
        (3900, 65),
        (4000, 80),
        (4150, 100),
    ];
    if mv <= CURVE[0].0 {
        return 0;
    }
    for w in CURVE.windows(2) {
        let ((v0, p0), (v1, p1)) = (w[0], w[1]);
        if mv <= v1 {
            return (p0 + (p1 - p0) * (mv - v0) / (v1 - v0)) as u8;
        }
    }
    100
}
