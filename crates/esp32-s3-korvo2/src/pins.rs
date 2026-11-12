// SPDX-License-Identifier: MIT OR Apache-2.0
//! GPIO assignment of the ESP32-S3-Korvo-2 V3.x (schematic V3.1.2, cross-checked
//! with Espressif's reference board configuration).

/// I2C SDA (ES8311, ES7210, TCA9554, camera, LCD touch).
pub const I2C_SDA: i32 = 17;
/// I2C SCL.
pub const I2C_SCL: i32 = 18;
/// I2S0 MCLK (ES8311 + ES7210).
pub const I2S_MCLK: i32 = 16;
/// I2S0 bit clock.
pub const I2S_BCLK: i32 = 9;
/// I2S0 word select (LRCK).
pub const I2S_WS: i32 = 45;
/// I2S0 data out, ESP32-S3 -> ES8311 DSDIN.
pub const I2S_DOUT: i32 = 8;
/// I2S0 data in, ES7210 SDOUT -> ESP32-S3.
pub const I2S_DIN: i32 = 10;
/// NS4150 power amplifier enable (high = on). Directly on GPIO48 since V3.1.
pub const PA_ENABLE: i32 = 48;
/// Button resistor ladder (ADC1 channel 4).
pub const BUTTONS_ADC: i32 = 5;
/// Battery voltage divider (ADC1 channel 5).
pub const BATTERY_ADC: i32 = 6;
/// BOOT button (shared with LCD SPI SDA).
pub const BOOT: i32 = 0;

/// 7-bit I2C address of the ES8311 codec (`ES8311_CODEC_DEFAULT_ADDR >> 1`).
pub const ES8311_ADDR: u16 = 0x18;
/// 7-bit I2C address of the ES7210 ADC (AD0 = AD1 = 0).
pub const ES7210_ADDR: u16 = 0x40;
/// 7-bit I2C address of the I/O expander when fitted as a TCA9554A (A0..A2 = 0),
/// as drawn in schematic V3.1.2.
pub const TCA9554A_ADDR: u16 = 0x38;
/// 7-bit I2C address of the I/O expander when fitted as a TCA9554 (A0..A2 = 0).
/// Observed on a V3.1 board; both addresses are probed.
pub const TCA9554_ADDR: u16 = 0x20;

/// I/O expander bits.
pub mod expander {
    /// P1: LCD_CTRL.
    pub const LCD_CTRL: u8 = 1;
    /// P2: LCD_RST.
    pub const LCD_RST: u8 = 2;
    /// P3: LCD_CS.
    pub const LCD_CS: u8 = 3;
    /// P4: TP_INT (touch interrupt, input).
    pub const TP_INT: u8 = 4;
    /// P5: PERI_PWR_ON (low = peripheral 3.3 V on; bypassed by a 0 Ω on V3.1).
    pub const PERI_PWR_ON: u8 = 5;
    /// P6: GP_LED1, red LED (high = on).
    pub const LED_RED: u8 = 6;
    /// P7: GP_LED2, blue LED (high = on).
    pub const LED_BLUE: u8 = 7;
}
