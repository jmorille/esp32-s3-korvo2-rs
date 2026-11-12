// SPDX-License-Identifier: MIT OR Apache-2.0
//! ESP-SR model partition.

use std::sync::Arc;

use crate::model_info::{self, ModelKind, WakeNetInfo};
use crate::{raw, Error};

/// Models stored in an ESP-SR model partition (WakeNet, VADNet, NSNet, MultiNet).
///
/// Cheap to clone; the underlying list (`srmodel_list_t`) is released when the
/// last clone and the last [`crate::Afe`] / [`crate::wakenet::WakeNet`] using it
/// are dropped.
#[derive(Clone)]
pub struct Models {
    pub(crate) inner: Arc<raw::RawModels>,
}

impl core::fmt::Debug for Models {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.names()).finish()
    }
}

impl Models {
    /// Load the model list from the partition with this label (usually `"model"`),
    /// via `esp_srmodel_init()`.
    pub fn load(partition_label: &str) -> Result<Self, Error> {
        Ok(Self {
            inner: Arc::new(raw::RawModels::init(partition_label)?),
        })
    }

    /// Names of all models, e.g. `["wn9_hiesp", "wn9_jarvis_tts", "vadnet1_medium"]`.
    pub fn names(&self) -> Vec<String> {
        self.inner.names()
    }

    /// Names of the models of one kind.
    pub fn names_of(&self, kind: ModelKind) -> Vec<String> {
        let names = self.names();
        model_info::filter_kind(names.iter().map(String::as_str), kind)
    }

    /// WakeNet model names (prefix `wn`, `ESP_WN_PREFIX`).
    pub fn wakenet_names(&self) -> Vec<String> {
        self.names_of(ModelKind::WakeNet)
    }

    /// Whether a model with this exact name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.names().iter().any(|n| n == name)
    }

    /// Raw ESP-SR info string of a model (type, version, words, thresholds).
    pub fn info(&self, name: &str) -> Option<String> {
        let idx = self.names().iter().position(|n| n == name)?;
        self.inner.info(idx)
    }

    /// Parsed info of a WakeNet model.
    pub fn wakenet_info(&self, name: &str) -> Option<WakeNetInfo> {
        WakeNetInfo::parse(&self.info(name)?)
    }

    /// Wake word(s) of a WakeNet model (e.g. `["Jarvis"]`).
    pub fn wake_words(&self, name: &str) -> Vec<String> {
        self.inner
            .wake_words(name)
            .map(|s| model_info::split_wake_words(&s))
            .unwrap_or_default()
    }

    /// First model whose name contains `prefix` and (if given) `keyword`, with
    /// the matching rule of `esp_srmodel_filter()`.
    /// E.g. `find("wn", Some("jarvis"))`, `find("vadnet", None)`.
    pub fn find(&self, prefix: &str, keyword: Option<&str>) -> Option<String> {
        let names = self.names();
        model_info::find(names.iter().map(String::as_str), prefix, keyword)
    }
}
