// SPDX-License-Identifier: MIT OR Apache-2.0
//! LEDs on the TCA9554(A) I/O expander.

use std::sync::{Arc, Mutex};

use crate::pins::{self, expander};
use crate::{ffi, Error, Expander};

/// TCA9554 registers.
const REG_OUTPUT: u8 = 0x01;
const REG_CONFIG: u8 = 0x03;

/// An LED on the I/O expander.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Led {
    /// Red LED (D7, expander P6).
    Red,
    /// Blue LED (D34, expander P7).
    Blue,
}

impl Led {
    fn bit(self) -> u8 {
        match self {
            Self::Red => 1 << expander::LED_RED,
            Self::Blue => 1 << expander::LED_BLUE,
        }
    }
}

/// LED controller. Cheap to clone and usable from several threads. If the I/O
/// expander does not answer, all operations are no-ops returning `Ok`.
#[derive(Clone)]
pub struct Leds {
    dev: Expander,
    out: Arc<Mutex<u8>>,
}

impl Leds {
    pub(crate) fn new(bus: &Arc<ffi::I2cBus>) -> Self {
        let addr = [pins::TCA9554A_ADDR, pins::TCA9554_ADDR]
            .into_iter()
            .find(|a| bus.probe(*a));
        // Outputs: P5 (PERI_PWR_ON, low = on), P6/P7 (LEDs, off). Others stay inputs.
        let out = 0u8;
        let dev = addr.and_then(|a| {
            let mut d = ffi::I2cDevice::new(bus, a, 100_000).ok()?;
            let outputs =
                (1 << expander::PERI_PWR_ON) | (1 << expander::LED_RED) | (1 << expander::LED_BLUE);
            d.write_reg(REG_OUTPUT, out).ok()?;
            let cfg = d.read_reg(REG_CONFIG).unwrap_or(0xff);
            d.write_reg(REG_CONFIG, cfg & !outputs).ok()?;
            log::info!("TCA9554 I/O expander at {a:#04x}");
            Some(d)
        });
        if dev.is_none() {
            log::warn!("TCA9554 I/O expander not found, LEDs disabled");
        }
        Self {
            dev: Arc::new(Mutex::new(dev)),
            out: Arc::new(Mutex::new(out)),
        }
    }

    /// Whether the I/O expander was found.
    pub fn available(&self) -> bool {
        self.dev.lock().map(|d| d.is_some()).unwrap_or(false)
    }

    /// Switch an LED on or off.
    pub fn set(&self, led: Led, on: bool) -> Result<(), Error> {
        let mut dev = self.dev.lock().unwrap_or_else(|e| e.into_inner());
        let Some(d) = dev.as_mut() else {
            return Ok(());
        };
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let new = if on {
            *out | led.bit()
        } else {
            *out & !led.bit()
        };
        d.write_reg(REG_OUTPUT, new)?;
        *out = new;
        Ok(())
    }

    /// Toggle an LED.
    pub fn toggle(&self, led: Led) -> Result<(), Error> {
        let on = self.out.lock().map(|o| *o & led.bit() == 0).unwrap_or(true);
        self.set(led, on)
    }
}
