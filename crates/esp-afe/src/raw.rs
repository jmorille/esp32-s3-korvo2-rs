// SPDX-License-Identifier: MIT OR Apache-2.0
//! The only module of this crate containing `unsafe` code.
//!
//! It owns the raw ESP-SR pointers and exposes small safe wrappers that the rest
//! of the crate builds on. Every `unsafe` block carries a `SAFETY:` comment.

use core::ffi::{c_char, CStr};
use std::ffi::CString;
use std::sync::Arc;

use esp_idf_sys::esp_sr as sr;

use crate::config::{AfeConfig, AgcMode, Toggle};
use crate::Error;

/// Owned `srmodel_list_t` (models found in a flash partition).
pub(crate) struct RawModels {
    ptr: *mut sr::srmodel_list_t,
}

// SAFETY: the model list is immutable after `esp_srmodel_init()` returns; it only
// holds pointers into the memory-mapped partition and heap strings that are never
// modified until `esp_srmodel_deinit()` (called once, from `Drop`).
unsafe impl Send for RawModels {}
// SAFETY: see above, all accesses after init are reads.
unsafe impl Sync for RawModels {}

impl RawModels {
    pub(crate) fn init(partition_label: &str) -> Result<Self, Error> {
        let label = CString::new(partition_label).map_err(|_| Error::InvalidName)?;
        // SAFETY: `label` is a valid NUL-terminated string that outlives the call;
        // the function returns either NULL or a heap-allocated list we now own.
        let ptr = unsafe { sr::esp_srmodel_init(label.as_ptr()) };
        if ptr.is_null() {
            return Err(Error::ModelsNotFound(partition_label.to_owned()));
        }
        Ok(Self { ptr })
    }

    fn len(&self) -> usize {
        // SAFETY: `ptr` is non-null and valid for the lifetime of `self`.
        let num = unsafe { (*self.ptr).num };
        num.max(0) as usize
    }

    /// Pointer to the C string at index `i` of a `char **` array owned by the list.
    fn string_at(&self, array: *mut *mut c_char, i: usize) -> Option<String> {
        if array.is_null() || i >= self.len() {
            return None;
        }
        // SAFETY: `array` has `num` entries (checked above) owned by the list,
        // each either NULL or a NUL-terminated string valid while `self` lives.
        unsafe {
            let s = *array.add(i);
            if s.is_null() {
                None
            } else {
                Some(CStr::from_ptr(s).to_string_lossy().into_owned())
            }
        }
    }

    pub(crate) fn names(&self) -> Vec<String> {
        // SAFETY: `ptr` is valid; reading a field.
        let arr = unsafe { (*self.ptr).model_name };
        (0..self.len())
            .filter_map(|i| self.string_at(arr, i))
            .collect()
    }

    pub(crate) fn info(&self, index: usize) -> Option<String> {
        // SAFETY: `ptr` is valid; reading a field.
        let arr = unsafe { (*self.ptr).model_info };
        self.string_at(arr, index)
    }

    /// Pointer to the list-owned C string of model `name` (stable while `self` lives).
    pub(crate) fn name_ptr(&self, name: &str) -> Result<*mut c_char, Error> {
        // SAFETY: `ptr` is valid; reading a field.
        let arr = unsafe { (*self.ptr).model_name };
        for i in 0..self.len() {
            if self.string_at(arr, i).as_deref() == Some(name) {
                // SAFETY: index checked by `string_at` returning Some.
                return Ok(unsafe { *arr.add(i) });
            }
        }
        Err(Error::ModelNotFound(name.to_owned()))
    }

    pub(crate) fn wake_words(&self, name: &str) -> Option<String> {
        let p = self.name_ptr(name).ok()?;
        // SAFETY: `p` is a model name owned by the list; the returned pointer is a
        // string owned by the list as well (or NULL).
        let w = unsafe { sr::esp_srmodel_get_wake_words(self.ptr, p) };
        if w.is_null() {
            None
        } else {
            // SAFETY: non-null NUL-terminated string from ESP-SR.
            Some(unsafe { CStr::from_ptr(w) }.to_string_lossy().into_owned())
        }
    }
}

impl Drop for RawModels {
    fn drop(&mut self) {
        // SAFETY: `ptr` came from `esp_srmodel_init` and is released exactly once.
        unsafe { sr::esp_srmodel_deinit(self.ptr) }
    }
}

/// Copy of an `afe_fetch_result_t`.
pub(crate) struct RawFetch {
    pub data: Vec<i16>,
    pub vad_cache: Vec<i16>,
    pub volume_db: f32,
    pub wake: Option<(i32, i32, i32)>,
    pub speech: bool,
    pub trigger_channel: i32,
    pub ringbuf_free_pct: f32,
}

/// Owned AFE instance (interface table + handle).
pub(crate) struct RawAfe {
    iface: *const sr::esp_afe_sr_iface_t,
    data: *mut sr::esp_afe_sr_data_t,
    _models: Option<Arc<RawModels>>,
}

// SAFETY: the AFE handle is a heap object not tied to the creating task. ESP-SR is
// designed for `feed()` and `fetch()` to run concurrently on two different tasks.
// The safe API guarantees that `feed` is only reached through `&mut` of a single
// owner (`Afe` or `AfeFeeder`) and `fetch`/control functions through `&mut` of a
// single owner (`Afe` or `AfeFetcher`), so no function is called concurrently with
// itself.
unsafe impl Send for RawAfe {}
// SAFETY: see above.
unsafe impl Sync for RawAfe {}

/// Read a function pointer from the interface table or fail.
macro_rules! op {
    ($self:ident, $name:ident) => {{
        // SAFETY: `iface` is a non-null static table returned by ESP-SR.
        let f = unsafe { (*$self.iface).$name };
        f.ok_or(Error::Unsupported(stringify!($name)))?
    }};
}

impl RawAfe {
    pub(crate) fn new(cfg: &AfeConfig, models: Option<Arc<RawModels>>) -> Result<Self, Error> {
        let fmt = CString::new(cfg.input_format.as_str()).map_err(|_| Error::InvalidName)?;
        let models_ptr = models.as_ref().map_or(core::ptr::null_mut(), |m| m.ptr);

        // Resolve model names to list-owned pointers *before* allocating the config.
        let vad_model = match &cfg.vad {
            Toggle::On(v) => match &v.model {
                Some(n) => Some(lookup(&models, n)?),
                None => None,
            },
            _ => None,
        };
        let wn = match &cfg.wakenet {
            Toggle::On((m1, m2)) => Some((
                lookup(&models, m1)?,
                match m2 {
                    Some(m) => lookup(&models, m)?,
                    None => core::ptr::null_mut(),
                },
            )),
            _ => None,
        };

        // SAFETY: `fmt` is NUL-terminated and outlives the call; `models_ptr` is
        // NULL or a live model list. Returns NULL or a heap config we own.
        let c = unsafe {
            sr::afe_config_init(
                fmt.as_ptr(),
                models_ptr,
                cfg.afe_type.raw() as _,
                cfg.mode.raw() as _,
            )
        };
        if c.is_null() {
            return Err(Error::Config);
        }
        // SAFETY: `c` is non-null, exclusively ours until `afe_config_free`.
        let c_ref = unsafe { &mut *c };

        match &cfg.aec {
            Toggle::On(a) => {
                c_ref.aec_init = true;
                c_ref.aec_mode = a.mode.raw() as _;
                c_ref.aec_nlp_level = a.nlp_level.raw() as _;
                if let Some(len) = a.filter_length {
                    c_ref.aec_filter_length = len as _;
                }
            }
            Toggle::Off => c_ref.aec_init = false,
            Toggle::Default => {}
        }
        if let Some(se) = cfg.se {
            c_ref.se_init = se;
        }
        if let Some(ns) = cfg.ns {
            c_ref.ns_init = ns;
        }
        match &cfg.vad {
            Toggle::On(v) => {
                c_ref.vad_init = true;
                c_ref.vad_mode = v.mode.raw() as _;
                c_ref.vad_min_speech_ms = v.min_speech_ms as _;
                c_ref.vad_min_noise_ms = v.min_noise_ms as _;
                c_ref.vad_model_name = vad_model.unwrap_or(core::ptr::null_mut());
            }
            Toggle::Off => c_ref.vad_init = false,
            Toggle::Default => {}
        }
        if let Some(t) = cfg.vad_energy_threshold {
            c_ref.vad_energy_threshold = t;
        }
        match &cfg.agc {
            Toggle::On(a) => {
                c_ref.agc_init = true;
                c_ref.agc_mode = match a.mode {
                    AgcMode::WebRtc => 0,
                    AgcMode::WakeNet => 1,
                } as _;
                c_ref.agc_compression_gain_db = a.compression_gain_db as _;
                c_ref.agc_target_level_dbfs = a.target_level_dbfs as _;
            }
            Toggle::Off => c_ref.agc_init = false,
            Toggle::Default => {}
        }
        match wn {
            Some((m1, m2)) => {
                c_ref.wakenet_init = true;
                c_ref.wakenet_model_name = m1;
                c_ref.wakenet_model_name_2 = m2;
                if let Some(mode) = cfg.wakenet_mode {
                    c_ref.wakenet_mode = mode.raw() as _;
                }
            }
            None => {
                if matches!(cfg.wakenet, Toggle::Off) {
                    c_ref.wakenet_init = false;
                    c_ref.wakenet_model_name = core::ptr::null_mut();
                    c_ref.wakenet_model_name_2 = core::ptr::null_mut();
                }
            }
        }
        c_ref.memory_alloc_mode = cfg.memory.raw() as _;
        if let Some(g) = cfg.linear_gain {
            c_ref.afe_linear_gain = g;
        }
        if let Some(core) = cfg.core {
            c_ref.afe_perferred_core = core as _;
        }
        if let Some(p) = cfg.priority {
            c_ref.afe_perferred_priority = p as _;
        }
        if let Some(n) = cfg.ringbuf_size {
            c_ref.afe_ringbuf_size = n as _;
        }

        // SAFETY: `c` is a valid config; the returned table is static.
        let iface = unsafe { sr::esp_afe_handle_from_config(c) };
        let data = if iface.is_null() {
            core::ptr::null_mut()
        } else {
            // SAFETY: `iface` is non-null and static.
            match unsafe { (*iface).create_from_config } {
                // SAFETY: `c` is a valid config; ESP-SR copies what it needs.
                Some(create) => unsafe { create(c) },
                None => core::ptr::null_mut(),
            }
        };
        // SAFETY: `c` came from `afe_config_init` and is freed exactly once. The
        // model name pointers we stored are owned by the model list, which ESP-SR
        // does not free here (same pattern as the ESP-SR examples).
        unsafe { sr::afe_config_free(c) };

        if iface.is_null() || data.is_null() {
            return Err(Error::Create);
        }
        Ok(Self {
            iface,
            data,
            _models: models,
        })
    }

    pub(crate) fn feed_chunk_size(&self) -> Result<usize, Error> {
        let f = op!(self, get_feed_chunksize);
        // SAFETY: `data` is a live AFE handle.
        Ok(unsafe { f(self.data) }.max(0) as usize)
    }

    pub(crate) fn fetch_chunk_size(&self) -> Result<usize, Error> {
        let f = op!(self, get_fetch_chunksize);
        // SAFETY: `data` is a live AFE handle.
        Ok(unsafe { f(self.data) }.max(0) as usize)
    }

    pub(crate) fn channel_num(&self) -> Result<usize, Error> {
        let f = op!(self, get_feed_channel_num);
        // SAFETY: `data` is a live AFE handle.
        Ok(unsafe { f(self.data) }.max(0) as usize)
    }

    pub(crate) fn sample_rate(&self) -> Result<u32, Error> {
        let f = op!(self, get_samp_rate);
        // SAFETY: `data` is a live AFE handle.
        Ok(unsafe { f(self.data) }.max(0) as u32)
    }

    /// `samples.len()` must be exactly `feed_chunk_size * channel_num` (checked by caller).
    pub(crate) fn feed(&self, samples: &[i16]) -> Result<(), Error> {
        let f = op!(self, feed);
        // SAFETY: `data` is live; `samples` holds one full feed chunk (length checked
        // by the caller), ESP-SR only reads that many samples and copies them.
        let r = unsafe { f(self.data, samples.as_ptr()) };
        if r < 0 {
            Err(Error::Feed(r))
        } else {
            Ok(())
        }
    }

    pub(crate) fn fetch(&self, ticks: u32) -> Result<RawFetch, Error> {
        let f = op!(self, fetch_with_delay);
        // SAFETY: `data` is live; the result points to AFE-owned memory valid until
        // the next fetch on this handle. We copy everything out before returning
        // and fetch is never called concurrently (see `Sync` impl).
        let r = unsafe { f(self.data, ticks as _) };
        if r.is_null() {
            return Err(Error::Timeout);
        }
        // SAFETY: non-null result from ESP-SR.
        let r = unsafe { &*r };
        if r.ret_value == esp_idf_sys::ESP_FAIL {
            return Err(Error::Timeout);
        }
        let copy = |p: *const i16, bytes: i32| -> Vec<i16> {
            if p.is_null() || bytes <= 0 {
                Vec::new()
            } else {
                let n = bytes as usize / core::mem::size_of::<i16>();
                // SAFETY: ESP-SR guarantees `p` points to `bytes` bytes of i16 data.
                unsafe { core::slice::from_raw_parts(p, n) }.to_vec()
            }
        };
        let wake = if r.wakeup_state == 1 {
            Some((r.wake_word_index, r.wakenet_model_index, r.wake_word_length))
        } else {
            None
        };
        Ok(RawFetch {
            data: copy(r.data, r.data_size),
            vad_cache: copy(r.vad_cache, r.vad_cache_size),
            volume_db: r.data_volume,
            wake,
            speech: r.vad_state == 1,
            trigger_channel: r.trigger_channel_id,
            ringbuf_free_pct: r.ringbuff_free_pct,
        })
    }

    pub(crate) fn reset_buffer(&self) -> Result<(), Error> {
        let f = op!(self, reset_buffer);
        // SAFETY: `data` is a live AFE handle.
        check(unsafe { f(self.data) }, "reset_buffer")
    }

    pub(crate) fn toggle(&self, feature: Feature, enable: bool) -> Result<(), Error> {
        let f = match (feature, enable) {
            (Feature::Aec, true) => op!(self, enable_aec),
            (Feature::Aec, false) => op!(self, disable_aec),
            (Feature::Se, true) => op!(self, enable_se),
            (Feature::Se, false) => op!(self, disable_se),
            (Feature::Ns, true) => op!(self, enable_ns),
            (Feature::Ns, false) => op!(self, disable_ns),
            (Feature::Vad, true) => op!(self, enable_vad),
            (Feature::Vad, false) => op!(self, disable_vad),
            (Feature::Agc, true) => op!(self, enable_agc),
            (Feature::Agc, false) => op!(self, disable_agc),
            (Feature::WakeNet, true) => op!(self, enable_wakenet),
            (Feature::WakeNet, false) => op!(self, disable_wakenet),
        };
        // SAFETY: `data` is a live AFE handle.
        check(unsafe { f(self.data) }, "enable/disable")
    }

    pub(crate) fn reset_vad(&self) -> Result<(), Error> {
        let f = op!(self, reset_vad);
        // SAFETY: `data` is a live AFE handle.
        check(unsafe { f(self.data) }, "reset_vad")
    }

    pub(crate) fn set_wakenet_threshold(&self, index: i32, threshold: f32) -> Result<(), Error> {
        let f = op!(self, set_wakenet_threshold);
        // SAFETY: `data` is a live AFE handle.
        check(
            unsafe { f(self.data, index, threshold) },
            "set_wakenet_threshold",
        )
    }

    pub(crate) fn print_pipeline(&self) -> Result<(), Error> {
        let f = op!(self, print_pipeline);
        // SAFETY: `data` is a live AFE handle.
        unsafe { f(self.data) };
        Ok(())
    }
}

impl Drop for RawAfe {
    fn drop(&mut self) {
        // SAFETY: `iface` is static and non-null; `data` is live and destroyed
        // exactly once, after both feeder and fetcher are gone (Arc).
        unsafe {
            if let Some(destroy) = (*self.iface).destroy {
                destroy(self.data);
            }
        }
    }
}

/// AFE stages that can be toggled at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// Acoustic echo cancellation.
    Aec,
    /// Speech enhancement (BSS / beamforming).
    Se,
    /// Noise suppression.
    Ns,
    /// Voice activity detection.
    Vad,
    /// Automatic gain control.
    Agc,
    /// Wake word engine.
    WakeNet,
}

fn lookup(models: &Option<Arc<RawModels>>, name: &str) -> Result<*mut c_char, Error> {
    match models {
        Some(m) => m.name_ptr(name),
        None => Err(Error::ModelNotFound(name.to_owned())),
    }
}

fn check(r: i32, what: &'static str) -> Result<(), Error> {
    if r < 0 {
        Err(Error::Op(what))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Standalone WakeNet (esp_wn_iface.h / esp_wn_models.h)
// ---------------------------------------------------------------------------

/// Owned WakeNet model instance (interface table + `model_iface_data_t`).
pub(crate) struct RawWakeNet {
    iface: *const sr::esp_wn_iface_t,
    data: *mut sr::model_iface_data_t,
    /// `detect()` takes a non-const pointer: feed it a private copy.
    scratch: Vec<i16>,
    _models: Arc<RawModels>,
}

// SAFETY: the model instance is a heap object not tied to the creating task, and
// every call goes through `&self`/`&mut self` of the single owner (`WakeNet`),
// which is not `Sync`, so calls are never concurrent.
unsafe impl Send for RawWakeNet {}

macro_rules! wn_op {
    ($self:ident, $name:ident) => {{
        // SAFETY: `iface` is a non-null static table returned by ESP-SR.
        let f = unsafe { (*$self.iface).$name };
        f.ok_or(Error::Unsupported(stringify!($name)))?
    }};
}

impl RawWakeNet {
    pub(crate) fn new(models: Arc<RawModels>, name: &str, det_mode: u32) -> Result<Self, Error> {
        // The name must be the list-owned string: `create()` resolves it against the
        // model list registered by `esp_srmodel_init()`.
        let name_ptr = models.name_ptr(name)?;
        // SAFETY: `name_ptr` is a NUL-terminated string owned by the live model list.
        let iface = unsafe { sr::esp_wn_handle_from_name(name_ptr) };
        if iface.is_null() {
            return Err(Error::ModelNotFound(name.to_owned()));
        }
        // SAFETY: `iface` non-null and static.
        let create = unsafe { (*iface).create }.ok_or(Error::Unsupported("create"))?;
        // SAFETY: `name_ptr` valid as above; `det_mode` is a valid `det_mode_t` value
        // produced by `DetMode::raw()`.
        let data = unsafe { create(name_ptr as *const core::ffi::c_void, det_mode as _) };
        if data.is_null() {
            return Err(Error::Create);
        }
        Ok(Self {
            iface,
            data,
            scratch: Vec::new(),
            _models: models,
        })
    }

    pub(crate) fn chunk_size(&self) -> Result<usize, Error> {
        let f = wn_op!(self, get_samp_chunksize);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data) }.max(0) as usize)
    }

    pub(crate) fn channel_num(&self) -> Result<usize, Error> {
        let f = wn_op!(self, get_channel_num);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data) }.max(1) as usize)
    }

    pub(crate) fn sample_rate(&self) -> Result<u32, Error> {
        let f = wn_op!(self, get_samp_rate);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data) }.max(0) as u32)
    }

    pub(crate) fn word_count(&self) -> Result<usize, Error> {
        let f = wn_op!(self, get_word_num);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data) }.max(0) as usize)
    }

    /// `index` starts at 1.
    pub(crate) fn word_name(&self, index: usize) -> Result<Option<String>, Error> {
        let f = wn_op!(self, get_word_name);
        // SAFETY: `data` is live; ESP-SR returns NULL or a string owned by the model.
        let p = unsafe { f(self.data, index as i32) };
        if p.is_null() {
            return Ok(None);
        }
        // SAFETY: non-null NUL-terminated string valid while the model lives; copied.
        Ok(Some(
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned(),
        ))
    }

    pub(crate) fn set_threshold(&mut self, index: usize, threshold: f32) -> Result<(), Error> {
        let f = wn_op!(self, set_det_threshold);
        // SAFETY: `data` is a live model instance.
        let r = unsafe { f(self.data, threshold, index as i32) };
        if r == 0 {
            Err(Error::Op("set_det_threshold"))
        } else {
            Ok(())
        }
    }

    pub(crate) fn threshold(&self, index: usize) -> Result<f32, Error> {
        let f = wn_op!(self, get_det_threshold);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data, index as i32) })
    }

    pub(crate) fn reset_threshold(&mut self) -> Result<(), Error> {
        let f = wn_op!(self, reset_det_threshold);
        // SAFETY: `data` is a live model instance.
        check(unsafe { f(self.data) } - 1, "reset_det_threshold")
    }

    pub(crate) fn start_point(&self) -> Result<i32, Error> {
        let f = wn_op!(self, get_start_point);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data) })
    }

    pub(crate) fn triggered_channel(&self) -> Result<i32, Error> {
        let f = wn_op!(self, get_triggered_channel);
        // SAFETY: `data` is a live model instance.
        Ok(unsafe { f(self.data) })
    }

    /// `samples.len()` must equal `chunk_size * channel_num` (checked by caller).
    /// Returns the raw detect result: > 0 = index of the detected word.
    pub(crate) fn detect(&mut self, samples: &[i16]) -> Result<i32, Error> {
        let f = wn_op!(self, detect);
        self.scratch.clear();
        self.scratch.extend_from_slice(samples);
        // SAFETY: `data` is live; `scratch` holds exactly one chunk (length checked
        // by the caller) and is exclusively borrowed for the call.
        let r = unsafe { f(self.data, self.scratch.as_mut_ptr()) };
        // `wakenet_state_t` is a C enum: its Rust type depends on the target.
        #[allow(clippy::unnecessary_cast)]
        Ok(r as i32)
    }

    pub(crate) fn clean(&mut self) -> Result<(), Error> {
        let f = wn_op!(self, clean);
        // SAFETY: `data` is a live model instance.
        unsafe { f(self.data) };
        Ok(())
    }
}

impl Drop for RawWakeNet {
    fn drop(&mut self) {
        // SAFETY: `iface` static; `data` live and destroyed exactly once.
        unsafe {
            if let Some(destroy) = (*self.iface).destroy {
                destroy(self.data);
            }
        }
    }
}
