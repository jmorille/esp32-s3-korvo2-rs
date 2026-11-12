// SPDX-License-Identifier: MIT OR Apache-2.0
//! The only module of this crate containing `unsafe` code: thin owners around the
//! ESP-IDF I2C master bus, the TCA9554 expander, I2S0 full duplex + esp_codec_dev
//! (ES8311 / ES7210) and the ADC1 button ladder. Every `unsafe` block carries a
//! `SAFETY:` comment.
//!
//! The codec bring-up mirrors Espressif's reference board code (see NOTICE):
//! I2S0 TX in standard (Philips) stereo mode to the ES8311, I2S0 RX in 4-slot TDM
//! from the ES7210, sharing MCLK/BCLK/WS, MCLK = 256 * fs.

use core::ffi::c_void;
use core::ptr;
use std::sync::Arc;

use esp_idf_sys as sys;
use esp_idf_sys::codec;

use crate::{pins, Error};

fn esp(code: sys::esp_err_t, what: &'static str) -> Result<(), Error> {
    if code == sys::ESP_OK {
        Ok(())
    } else {
        Err(Error::Esp { code, what })
    }
}

fn codec_ok(code: i32, what: &'static str) -> Result<(), Error> {
    // esp_codec_dev returns ESP_CODEC_DEV_OK (0) or a negative error.
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Codec { code, what })
    }
}

// ---------------------------------------------------------------------------
// I2C
// ---------------------------------------------------------------------------

/// Owned I2C master bus (`i2c_master_bus_handle_t`).
pub(crate) struct I2cBus {
    pub(crate) handle: sys::i2c_master_bus_handle_t,
}

// SAFETY: the ESP-IDF I2C master driver serialises transactions with an internal
// bus lock, so the handle may be used from any task.
unsafe impl Send for I2cBus {}
// SAFETY: see above.
unsafe impl Sync for I2cBus {}

impl I2cBus {
    pub(crate) fn new(port: i32) -> Result<Self, Error> {
        let mut cfg = sys::i2c_master_bus_config_t {
            i2c_port: port,
            sda_io_num: pins::I2C_SDA,
            scl_io_num: pins::I2C_SCL,
            __bindgen_anon_1: sys::i2c_master_bus_config_t__bindgen_ty_1 {
                clk_source: sys::soc_periph_i2c_clk_src_t_I2C_CLK_SRC_DEFAULT,
            },
            glitch_ignore_cnt: 7,
            ..Default::default()
        };
        cfg.flags.set_enable_internal_pullup(1);
        let mut handle = ptr::null_mut();
        // SAFETY: `cfg` and `handle` are valid for the duration of the call.
        esp(
            unsafe { sys::i2c_new_master_bus(&cfg, &mut handle) },
            "i2c_new_master_bus",
        )?;
        Ok(Self { handle })
    }

    pub(crate) fn probe(&self, addr: u16) -> bool {
        // SAFETY: `handle` is a live bus.
        unsafe { sys::i2c_master_probe(self.handle, addr, 50) == sys::ESP_OK }
    }
}

impl Drop for I2cBus {
    fn drop(&mut self) {
        // SAFETY: `I2cDevice`s hold an `Arc` to the bus and the codec control
        // interfaces are released before it (field order of `crate::Shared`), so no
        // device remains when the bus is deleted.
        unsafe { sys::i2c_del_master_bus(self.handle) };
    }
}

/// A 7-bit I2C device on the bus.
pub(crate) struct I2cDevice {
    handle: sys::i2c_master_dev_handle_t,
    /// Keeps the bus alive while the device exists.
    _bus: Arc<I2cBus>,
}

// SAFETY: see `I2cBus`; device transactions take the bus lock.
unsafe impl Send for I2cDevice {}

impl I2cDevice {
    pub(crate) fn new(bus: &Arc<I2cBus>, addr: u16, speed_hz: u32) -> Result<Self, Error> {
        let cfg = sys::i2c_device_config_t {
            dev_addr_length: sys::i2c_addr_bit_len_t_I2C_ADDR_BIT_LEN_7,
            device_address: addr,
            scl_speed_hz: speed_hz,
            ..Default::default()
        };
        let mut handle = ptr::null_mut();
        // SAFETY: `bus.handle` is live, `cfg`/`handle` valid for the call.
        esp(
            unsafe { sys::i2c_master_bus_add_device(bus.handle, &cfg, &mut handle) },
            "i2c_master_bus_add_device",
        )?;
        Ok(Self {
            handle,
            _bus: bus.clone(),
        })
    }

    pub(crate) fn write_reg(&mut self, reg: u8, val: u8) -> Result<(), Error> {
        let buf = [reg, val];
        // SAFETY: `buf` is valid for 2 bytes during the blocking call.
        esp(
            unsafe { sys::i2c_master_transmit(self.handle, buf.as_ptr(), buf.len(), 100) },
            "i2c_master_transmit",
        )
    }

    pub(crate) fn read_reg(&mut self, reg: u8) -> Result<u8, Error> {
        let mut val = 0u8;
        // SAFETY: `reg`/`val` are valid for the blocking call.
        esp(
            unsafe { sys::i2c_master_transmit_receive(self.handle, &reg, 1, &mut val, 1, 100) },
            "i2c_master_transmit_receive",
        )?;
        Ok(val)
    }
}

impl Drop for I2cDevice {
    fn drop(&mut self) {
        // SAFETY: `handle` was added to a bus that outlives this device.
        unsafe { sys::i2c_master_bus_rm_device(self.handle) };
    }
}

// ---------------------------------------------------------------------------
// I2S0 + esp_codec_dev
// ---------------------------------------------------------------------------

/// I2S0 duplex channels, codec interfaces and the two esp_codec_dev devices.
pub(crate) struct Audio {
    tx: sys::i2s_chan_handle_t,
    rx: sys::i2s_chan_handle_t,
    data_if: *const codec::audio_codec_data_if_t,
    gpio_if: *const codec::audio_codec_gpio_if_t,
    out_ctrl: *const codec::audio_codec_ctrl_if_t,
    in_ctrl: *const codec::audio_codec_ctrl_if_t,
    out_codec: *const codec::audio_codec_if_t,
    in_codec: *const codec::audio_codec_if_t,
    pub(crate) out_dev: codec::esp_codec_dev_handle_t,
    pub(crate) in_dev: codec::esp_codec_dev_handle_t,
}

// SAFETY: all handles are heap objects of ESP-IDF / esp_codec_dev that are not tied
// to the creating task. The input device is only used through `&mut Mic` and the
// output device only through `&mut Speaker` (one owner each); the I2S driver
// supports concurrent RX and TX on the two channel handles.
unsafe impl Send for Audio {}
// SAFETY: see above.
unsafe impl Sync for Audio {}

/// Settings for [`Audio::new`].
pub(crate) struct AudioParams {
    pub sample_rate: u32,
    pub pa_voltage: f32,
}

impl Audio {
    pub(crate) fn new(bus: &I2cBus, p: &AudioParams) -> Result<Self, Error> {
        let mut a = Self {
            tx: ptr::null_mut(),
            rx: ptr::null_mut(),
            data_if: ptr::null(),
            gpio_if: ptr::null(),
            out_ctrl: ptr::null(),
            in_ctrl: ptr::null(),
            out_codec: ptr::null(),
            in_codec: ptr::null(),
            out_dev: ptr::null_mut(),
            in_dev: ptr::null_mut(),
        };
        // On error, `a` is dropped and releases whatever was created so far.
        a.init_i2s(p.sample_rate)?;
        a.init_codecs(bus, p)?;
        Ok(a)
    }

    fn init_i2s(&mut self, rate: u32) -> Result<(), Error> {
        let chan_cfg = sys::i2s_chan_config_t {
            id: sys::i2s_port_t_I2S_NUM_0,
            role: sys::i2s_role_t_I2S_ROLE_MASTER,
            dma_desc_num: 6,
            dma_frame_num: 240,
            __bindgen_anon_1: sys::i2s_chan_config_t__bindgen_ty_1 {
                auto_clear_after_cb: true,
            },
            auto_clear_before_cb: false,
            intr_priority: 0,
            ..Default::default()
        };
        // SAFETY: config and out-pointers are valid for the call.
        esp(
            unsafe { sys::i2s_new_channel(&chan_cfg, &mut self.tx, &mut self.rx) },
            "i2s_new_channel",
        )?;

        // TX: standard Philips, 16-bit stereo, to the ES8311.
        let mut std_cfg = sys::i2s_std_config_t::default();
        std_cfg.clk_cfg.sample_rate_hz = rate;
        std_cfg.clk_cfg.clk_src = sys::soc_periph_i2s_clk_src_t_I2S_CLK_SRC_DEFAULT;
        std_cfg.clk_cfg.mclk_multiple = sys::i2s_mclk_multiple_t_I2S_MCLK_MULTIPLE_256;
        std_cfg.slot_cfg.data_bit_width = sys::i2s_data_bit_width_t_I2S_DATA_BIT_WIDTH_16BIT;
        std_cfg.slot_cfg.slot_bit_width = sys::i2s_slot_bit_width_t_I2S_SLOT_BIT_WIDTH_AUTO;
        std_cfg.slot_cfg.slot_mode = sys::i2s_slot_mode_t_I2S_SLOT_MODE_STEREO;
        std_cfg.slot_cfg.slot_mask = sys::i2s_std_slot_mask_t_I2S_STD_SLOT_BOTH;
        std_cfg.slot_cfg.ws_width = 16;
        std_cfg.slot_cfg.ws_pol = false;
        std_cfg.slot_cfg.bit_shift = true;
        std_cfg.slot_cfg.left_align = true;
        std_cfg.gpio_cfg.mclk = pins::I2S_MCLK;
        std_cfg.gpio_cfg.bclk = pins::I2S_BCLK;
        std_cfg.gpio_cfg.ws = pins::I2S_WS;
        std_cfg.gpio_cfg.dout = pins::I2S_DOUT;
        std_cfg.gpio_cfg.din = sys::gpio_num_t_GPIO_NUM_NC;

        // RX: 4-slot TDM, 16-bit, from the ES7210 (slots = MIC1, MIC3, MIC2, MIC4).
        let mut tdm_cfg = sys::i2s_tdm_config_t::default();
        tdm_cfg.clk_cfg.sample_rate_hz = rate;
        tdm_cfg.clk_cfg.clk_src = sys::soc_periph_i2s_clk_src_t_I2S_CLK_SRC_DEFAULT;
        tdm_cfg.clk_cfg.mclk_multiple = sys::i2s_mclk_multiple_t_I2S_MCLK_MULTIPLE_256;
        tdm_cfg.clk_cfg.bclk_div = 8;
        tdm_cfg.slot_cfg.data_bit_width = sys::i2s_data_bit_width_t_I2S_DATA_BIT_WIDTH_16BIT;
        tdm_cfg.slot_cfg.slot_bit_width = sys::i2s_slot_bit_width_t_I2S_SLOT_BIT_WIDTH_AUTO;
        tdm_cfg.slot_cfg.slot_mode = sys::i2s_slot_mode_t_I2S_SLOT_MODE_STEREO;
        tdm_cfg.slot_cfg.slot_mask = sys::i2s_tdm_slot_mask_t_I2S_TDM_SLOT0
            | sys::i2s_tdm_slot_mask_t_I2S_TDM_SLOT1
            | sys::i2s_tdm_slot_mask_t_I2S_TDM_SLOT2
            | sys::i2s_tdm_slot_mask_t_I2S_TDM_SLOT3;
        tdm_cfg.slot_cfg.ws_width = 0; // I2S_TDM_AUTO_WS_WIDTH
        tdm_cfg.slot_cfg.ws_pol = false;
        tdm_cfg.slot_cfg.bit_shift = true;
        tdm_cfg.slot_cfg.left_align = false;
        tdm_cfg.slot_cfg.skip_mask = false;
        tdm_cfg.slot_cfg.total_slot = 0; // I2S_TDM_AUTO_SLOT_NUM
        tdm_cfg.gpio_cfg.mclk = pins::I2S_MCLK;
        tdm_cfg.gpio_cfg.bclk = pins::I2S_BCLK;
        tdm_cfg.gpio_cfg.ws = pins::I2S_WS;
        tdm_cfg.gpio_cfg.dout = sys::gpio_num_t_GPIO_NUM_NC;
        tdm_cfg.gpio_cfg.din = pins::I2S_DIN;

        // SAFETY: channels were created above; configs are valid for the calls.
        unsafe {
            esp(
                sys::i2s_channel_init_std_mode(self.tx, &std_cfg),
                "i2s_channel_init_std_mode",
            )?;
            esp(
                sys::i2s_channel_init_tdm_mode(self.rx, &tdm_cfg),
                "i2s_channel_init_tdm_mode",
            )?;
            esp(sys::i2s_channel_enable(self.tx), "i2s_channel_enable(tx)")?;
            esp(sys::i2s_channel_enable(self.rx), "i2s_channel_enable(rx)")?;
        }
        Ok(())
    }

    fn init_codecs(&mut self, bus: &I2cBus, p: &AudioParams) -> Result<(), Error> {
        let mut i2s_cfg = codec::audio_codec_i2s_cfg_t {
            port: 0,
            rx_handle: self.rx as *mut c_void,
            tx_handle: self.tx as *mut c_void,
            ..Default::default()
        };
        // SAFETY: `i2s_cfg` is valid; the returned interface references the I2S
        // channels, which outlive it (released after it in `Drop`).
        self.data_if = unsafe { codec::audio_codec_new_i2s_data(&mut i2s_cfg) };
        if self.data_if.is_null() {
            return Err(Error::Init("audio_codec_new_i2s_data"));
        }
        // SAFETY: no arguments; returns a heap interface or NULL.
        self.gpio_if = unsafe { codec::audio_codec_new_gpio() };
        if self.gpio_if.is_null() {
            return Err(Error::Init("audio_codec_new_gpio"));
        }

        // ES8311 (DAC / speaker path). Addresses are 8-bit as expected by esp_codec_dev.
        let mut i2c_cfg = codec::audio_codec_i2c_cfg_t {
            port: 0,
            addr: codec::ES8311_CODEC_DEFAULT_ADDR as u8,
            bus_handle: bus.handle as *mut c_void,
            ..Default::default()
        };
        // SAFETY: `i2c_cfg` valid; the bus outlives the control interface.
        self.out_ctrl = unsafe { codec::audio_codec_new_i2c_ctrl(&mut i2c_cfg) };
        if self.out_ctrl.is_null() {
            return Err(Error::Init("audio_codec_new_i2c_ctrl(es8311)"));
        }
        let mut es8311 = codec::es8311_codec_cfg_t {
            ctrl_if: self.out_ctrl,
            gpio_if: self.gpio_if,
            codec_mode: codec::esp_codec_dec_work_mode_t_ESP_CODEC_DEV_WORK_MODE_DAC,
            pa_pin: pins::PA_ENABLE as i16,
            pa_reverted: false,
            master_mode: false,
            use_mclk: true,
            ..Default::default()
        };
        es8311.hw_gain.pa_voltage = p.pa_voltage;
        es8311.hw_gain.codec_dac_voltage = 3.3;
        // SAFETY: `es8311` valid; referenced interfaces outlive the codec.
        self.out_codec = unsafe { codec::es8311_codec_new(&mut es8311) };
        if self.out_codec.is_null() {
            return Err(Error::Init("es8311_codec_new"));
        }
        let mut dev_cfg = codec::esp_codec_dev_cfg_t {
            dev_type: codec::esp_codec_dev_type_t_ESP_CODEC_DEV_TYPE_OUT,
            codec_if: self.out_codec,
            data_if: self.data_if,
        };
        // SAFETY: `dev_cfg` valid; interfaces outlive the device.
        self.out_dev = unsafe { codec::esp_codec_dev_new(&mut dev_cfg) };
        if self.out_dev.is_null() {
            return Err(Error::Init("esp_codec_dev_new(out)"));
        }

        // ES7210 (4-channel ADC: MIC1, MIC2 = microphones, MIC3 = AEC reference).
        i2c_cfg.addr = codec::ES7210_CODEC_DEFAULT_ADDR as u8;
        // SAFETY: as above.
        self.in_ctrl = unsafe { codec::audio_codec_new_i2c_ctrl(&mut i2c_cfg) };
        if self.in_ctrl.is_null() {
            return Err(Error::Init("audio_codec_new_i2c_ctrl(es7210)"));
        }
        let mut es7210 = codec::es7210_codec_cfg_t {
            ctrl_if: self.in_ctrl,
            // ES7210_SEL_MIC1 | MIC2 | MIC3 | MIC4 (casted macros are not bindgen'd).
            mic_selected: 0b1111,
            ..Default::default()
        };
        // SAFETY: as above.
        self.in_codec = unsafe { codec::es7210_codec_new(&mut es7210) };
        if self.in_codec.is_null() {
            return Err(Error::Init("es7210_codec_new"));
        }
        dev_cfg.dev_type = codec::esp_codec_dev_type_t_ESP_CODEC_DEV_TYPE_IN;
        dev_cfg.codec_if = self.in_codec;
        // SAFETY: as above.
        self.in_dev = unsafe { codec::esp_codec_dev_new(&mut dev_cfg) };
        if self.in_dev.is_null() {
            return Err(Error::Init("esp_codec_dev_new(in)"));
        }
        Ok(())
    }

    /// Open the output (mono, 16-bit) and input (4 TDM slots, 16-bit) devices.
    pub(crate) fn open(&self, rate: u32) -> Result<(), Error> {
        let mut out_fs = codec::esp_codec_dev_sample_info_t {
            bits_per_sample: 16,
            channel: 1,
            channel_mask: 0,
            sample_rate: rate,
            mclk_multiple: 0,
        };
        let mut in_fs = codec::esp_codec_dev_sample_info_t {
            bits_per_sample: 16,
            channel: 4,
            channel_mask: 0,
            sample_rate: rate,
            mclk_multiple: 0,
        };
        // SAFETY: devices are live; sample infos valid for the calls.
        unsafe {
            codec_ok(
                codec::esp_codec_dev_open(self.out_dev, &mut out_fs),
                "esp_codec_dev_open(out)",
            )?;
            codec_ok(
                codec::esp_codec_dev_open(self.in_dev, &mut in_fs),
                "esp_codec_dev_open(in)",
            )?;
        }
        Ok(())
    }

    pub(crate) fn read(&self, buf: &mut [i16]) -> Result<(), Error> {
        let bytes = core::mem::size_of_val(buf) as i32;
        // SAFETY: `buf` is valid for `bytes` bytes of writes; blocking call; the
        // input device is used by a single owner (`Mic`).
        codec_ok(
            unsafe { codec::esp_codec_dev_read(self.in_dev, buf.as_mut_ptr().cast(), bytes) },
            "esp_codec_dev_read",
        )
    }

    pub(crate) fn write(&self, buf: &[i16]) -> Result<(), Error> {
        let bytes = core::mem::size_of_val(buf) as i32;
        // SAFETY: esp_codec_dev only reads `bytes` bytes from the pointer (the C
        // prototype is not const-qualified); single owner (`Speaker`).
        codec_ok(
            unsafe { codec::esp_codec_dev_write(self.out_dev, buf.as_ptr() as *mut c_void, bytes) },
            "esp_codec_dev_write",
        )
    }

    pub(crate) fn set_out_volume(&self, vol: i32) -> Result<(), Error> {
        // SAFETY: device is live.
        codec_ok(
            unsafe { codec::esp_codec_dev_set_out_vol(self.out_dev, vol) },
            "esp_codec_dev_set_out_vol",
        )
    }

    pub(crate) fn set_out_mute(&self, mute: bool) -> Result<(), Error> {
        // SAFETY: device is live.
        codec_ok(
            unsafe { codec::esp_codec_dev_set_out_mute(self.out_dev, mute) },
            "esp_codec_dev_set_out_mute",
        )
    }

    /// `mask` uses physical ES7210 MIC numbering (bit 0 = MIC1 ... bit 3 = MIC4).
    pub(crate) fn set_in_gain(&self, mask: u16, db: f32) -> Result<(), Error> {
        // SAFETY: device is live.
        codec_ok(
            unsafe { codec::esp_codec_dev_set_in_channel_gain(self.in_dev, mask, db) },
            "esp_codec_dev_set_in_channel_gain",
        )
    }

    pub(crate) fn set_in_mute(&self, mute: bool) -> Result<(), Error> {
        // SAFETY: device is live.
        codec_ok(
            unsafe { codec::esp_codec_dev_set_in_mute(self.in_dev, mute) },
            "esp_codec_dev_set_in_mute",
        )
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        // SAFETY: every pointer is either NULL or a live object created in `new`,
        // released exactly once, devices first, then codecs, control/data/gpio
        // interfaces and finally the I2S channels they reference.
        unsafe {
            for dev in [self.out_dev, self.in_dev] {
                if !dev.is_null() {
                    codec::esp_codec_dev_close(dev);
                    codec::esp_codec_dev_delete(dev);
                }
            }
            for c in [self.out_codec, self.in_codec] {
                if !c.is_null() {
                    codec::audio_codec_delete_codec_if(c);
                }
            }
            for c in [self.out_ctrl, self.in_ctrl] {
                if !c.is_null() {
                    codec::audio_codec_delete_ctrl_if(c);
                }
            }
            if !self.gpio_if.is_null() {
                codec::audio_codec_delete_gpio_if(self.gpio_if);
            }
            if !self.data_if.is_null() {
                codec::audio_codec_delete_data_if(self.data_if);
            }
            for ch in [self.tx, self.rx] {
                if !ch.is_null() {
                    sys::i2s_channel_disable(ch);
                    sys::i2s_del_channel(ch);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ADC1 one-shot (GPIO5 = buttons, ch 4; GPIO6 = battery, ch 5)
// ---------------------------------------------------------------------------

/// The ADC1 one-shot unit, shared by all ADC1 channels.
pub(crate) struct Adc1 {
    unit: sys::adc_oneshot_unit_handle_t,
}

// SAFETY: the one-shot driver serialises conversions on a unit internally, so
// the handle may be shared between tasks.
unsafe impl Send for Adc1 {}
// SAFETY: see above.
unsafe impl Sync for Adc1 {}

impl Adc1 {
    pub(crate) fn new() -> Result<Arc<Self>, Error> {
        let unit_cfg = sys::adc_oneshot_unit_init_cfg_t {
            unit_id: sys::adc_unit_t_ADC_UNIT_1,
            ..Default::default()
        };
        let mut unit = ptr::null_mut();
        // SAFETY: config and out-pointer valid for the call.
        esp(
            unsafe { sys::adc_oneshot_new_unit(&unit_cfg, &mut unit) },
            "adc_oneshot_new_unit",
        )?;
        Ok(Arc::new(Self { unit }))
    }
}

impl Drop for Adc1 {
    fn drop(&mut self) {
        // SAFETY: all channels hold an `Arc<Adc1>`, so none is left; released once.
        unsafe { sys::adc_oneshot_del_unit(self.unit) };
    }
}

/// One calibrated ADC1 channel (12 dB attenuation, 0 – ~3.1 V).
pub(crate) struct AdcChannel {
    adc: Arc<Adc1>,
    chan: sys::adc_channel_t,
    cali: sys::adc_cali_handle_t,
}

// SAFETY: handles are not tied to a task; used through `&mut self` only.
unsafe impl Send for AdcChannel {}

/// ADC1 channel of GPIO5 (buttons).
pub(crate) const ADC_CH_BUTTONS: u32 = sys::adc_channel_t_ADC_CHANNEL_4;
/// ADC1 channel of GPIO6 (battery).
pub(crate) const ADC_CH_BATTERY: u32 = sys::adc_channel_t_ADC_CHANNEL_5;

impl AdcChannel {
    pub(crate) fn new(adc: &Arc<Adc1>, chan: u32) -> Result<Self, Error> {
        let chan_cfg = sys::adc_oneshot_chan_cfg_t {
            atten: sys::adc_atten_t_ADC_ATTEN_DB_12,
            bitwidth: sys::adc_bitwidth_t_ADC_BITWIDTH_DEFAULT,
        };
        // SAFETY: `unit` is live; config valid.
        esp(
            unsafe { sys::adc_oneshot_config_channel(adc.unit, chan, &chan_cfg) },
            "adc_oneshot_config_channel",
        )?;
        let mut s = Self {
            adc: adc.clone(),
            chan,
            cali: ptr::null_mut(),
        };
        let cali_cfg = sys::adc_cali_curve_fitting_config_t {
            unit_id: sys::adc_unit_t_ADC_UNIT_1,
            chan,
            atten: sys::adc_atten_t_ADC_ATTEN_DB_12,
            bitwidth: sys::adc_bitwidth_t_ADC_BITWIDTH_DEFAULT,
        };
        // SAFETY: config and out-pointer valid. Failure (no eFuse calibration) is
        // tolerated: we fall back to a linear conversion.
        if unsafe { sys::adc_cali_create_scheme_curve_fitting(&cali_cfg, &mut s.cali) }
            != sys::ESP_OK
        {
            s.cali = ptr::null_mut();
        }
        Ok(s)
    }

    /// Voltage on the pin in millivolts.
    pub(crate) fn read_mv(&mut self) -> Result<i32, Error> {
        let mut v = 0i32;
        if self.cali.is_null() {
            // SAFETY: `unit` is live; `v` valid.
            esp(
                unsafe { sys::adc_oneshot_read(self.adc.unit, self.chan, &mut v) },
                "adc_oneshot_read",
            )?;
            Ok(v * 3100 / 4095)
        } else {
            // SAFETY: `unit` and `cali` are live; `v` valid.
            esp(
                unsafe {
                    sys::adc_oneshot_get_calibrated_result(
                        self.adc.unit,
                        self.cali,
                        self.chan,
                        &mut v,
                    )
                },
                "adc_oneshot_get_calibrated_result",
            )?;
            Ok(v)
        }
    }
}

impl Drop for AdcChannel {
    fn drop(&mut self) {
        if !self.cali.is_null() {
            // SAFETY: created in `new`, released once.
            unsafe { sys::adc_cali_delete_scheme_curve_fitting(self.cali) };
        }
    }
}
