// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure-Rust helpers to interpret ESP-SR model names and info strings.
//!
//! Host-testable (no ESP-IDF dependency).

/// Prefix of WakeNet model names (`ESP_WN_PREFIX` in `esp_wn_models.h`).
pub const WAKENET_PREFIX: &str = "wn";

/// Kind of an ESP-SR model, derived from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// Wake word engine (`wn9_hiesp`, `wn9_jarvis_tts`, `wn10_...`).
    WakeNet,
    /// Speech command recognition (`mn5q8_en`, `mn7_cn`...).
    MultiNet,
    /// Neural voice activity detection (`vadnet1_medium`).
    VadNet,
    /// Neural noise suppression (`nsnet2`).
    NsNet,
    /// Anything else.
    Other,
}

impl ModelKind {
    /// Classify a model by its name.
    pub fn of(name: &str) -> Self {
        if name.starts_with("vadnet") {
            Self::VadNet
        } else if name.starts_with("nsnet") {
            Self::NsNet
        } else if name.starts_with(WAKENET_PREFIX) {
            Self::WakeNet
        } else if name.starts_with("mn") {
            Self::MultiNet
        } else {
            Self::Other
        }
    }
}

/// Keep the names of a given kind, preserving order.
pub fn filter_kind<'a, I>(names: I, kind: ModelKind) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    names
        .into_iter()
        .filter(|n| ModelKind::of(n) == kind)
        .map(str::to_owned)
        .collect()
}

/// First name containing `prefix` and, if given, `keyword` — same matching
/// rule as `esp_srmodel_filter()`.
pub fn find<'a, I>(names: I, prefix: &str, keyword: Option<&str>) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    names
        .into_iter()
        .find(|n| n.contains(prefix) && keyword.is_none_or(|k| n.contains(k)))
        .map(str::to_owned)
}

/// Split the `;`-joined wake word list returned by `esp_srmodel_get_wake_words()`.
pub fn split_wake_words(s: &str) -> Vec<String> {
    s.split(';')
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// One wake word described in a WakeNet info string.
#[derive(Debug, Clone, PartialEq)]
pub struct WakeWordInfo {
    /// Word as written in the info string (e.g. `"Hi,ESP"`).
    pub word: String,
    /// Detection window (model-specific units).
    pub window_ms: Option<u32>,
    /// Detection threshold of the "normal" mode.
    pub threshold_normal: Option<f32>,
    /// Detection threshold of the "aggressive" mode.
    pub threshold_aggressive: Option<f32>,
}

/// Parsed WakeNet info string (`srmodel_list_t::model_info`), whose format is
/// `(model type)_(version)_(word1)_(window)_(threshold1)_(threshold2)_(word2...)`,
/// e.g. `"wakeNet9_v1h24_Hi,ESP_3_0.63_0.635"`.
#[derive(Debug, Clone, PartialEq)]
pub struct WakeNetInfo {
    /// Model type, e.g. `"wakeNet9"`.
    pub model_type: String,
    /// Version, e.g. `"v1h24"`.
    pub version: String,
    /// Wake words.
    pub words: Vec<WakeWordInfo>,
}

impl WakeNetInfo {
    /// Parse an info string. Returns `None` if it does not have at least a type,
    /// a version and one word.
    pub fn parse(info: &str) -> Option<Self> {
        let mut parts = info.trim().split('_');
        let model_type = parts.next()?.to_owned();
        let version = parts.next()?.to_owned();
        let rest: Vec<&str> = parts.collect();
        if rest.is_empty() || model_type.is_empty() {
            return None;
        }
        let words = rest
            .chunks(4)
            .map(|c| WakeWordInfo {
                word: c[0].to_owned(),
                window_ms: c.get(1).and_then(|v| v.parse().ok()),
                threshold_normal: c.get(2).and_then(|v| v.parse().ok()),
                threshold_aggressive: c.get(3).and_then(|v| v.parse().ok()),
            })
            .collect();
        Some(Self {
            model_type,
            version,
            words,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &[
        "wn9_hiesp",
        "wn9_jarvis_tts",
        "vadnet1_medium",
        "mn5q8_en",
        "nsnet2",
    ];

    #[test]
    fn kinds() {
        assert_eq!(ModelKind::of("wn9_hiesp"), ModelKind::WakeNet);
        assert_eq!(ModelKind::of("vadnet1_medium"), ModelKind::VadNet);
        assert_eq!(ModelKind::of("nsnet2"), ModelKind::NsNet);
        assert_eq!(ModelKind::of("mn5q8_en"), ModelKind::MultiNet);
        assert_eq!(ModelKind::of("foo"), ModelKind::Other);
    }

    #[test]
    fn filter_wakenet() {
        assert_eq!(
            filter_kind(NAMES.iter().copied(), ModelKind::WakeNet),
            vec!["wn9_hiesp", "wn9_jarvis_tts"]
        );
    }

    #[test]
    fn find_like_srmodel_filter() {
        assert_eq!(
            find(NAMES.iter().copied(), "wn", Some("jarvis")).as_deref(),
            Some("wn9_jarvis_tts")
        );
        assert_eq!(
            find(NAMES.iter().copied(), "wn", None).as_deref(),
            Some("wn9_hiesp")
        );
        assert_eq!(find(NAMES.iter().copied(), "wn", Some("alexa")), None);
    }

    #[test]
    fn wake_words() {
        assert_eq!(split_wake_words("Jarvis"), vec!["Jarvis"]);
        assert_eq!(
            split_wake_words("hi esp; hi lexin;"),
            vec!["hi esp", "hi lexin"]
        );
        assert!(split_wake_words("").is_empty());
    }

    #[test]
    fn parse_info() {
        let i = WakeNetInfo::parse("wakeNet9_v1h24_Hi,ESP_3_0.63_0.635").unwrap();
        assert_eq!(i.model_type, "wakeNet9");
        assert_eq!(i.version, "v1h24");
        assert_eq!(i.words.len(), 1);
        assert_eq!(i.words[0].word, "Hi,ESP");
        assert_eq!(i.words[0].window_ms, Some(3));
        assert_eq!(i.words[0].threshold_normal, Some(0.63));
        assert_eq!(i.words[0].threshold_aggressive, Some(0.635));

        let two = WakeNetInfo::parse("wakeNet9_v1h24_A_3_0.6_0.7_B_3_0.5_0.55").unwrap();
        assert_eq!(two.words.len(), 2);
        assert_eq!(two.words[1].word, "B");
        assert!(WakeNetInfo::parse("wakeNet9").is_none());
    }
}
