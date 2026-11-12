// SPDX-License-Identifier: MIT OR Apache-2.0
//! Builder for the ESP-SR `afe_config_t` structure.

use crate::Error;

/// Scenario the AFE pipeline is tuned for (`afe_type_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AfeType {
    /// Speech recognition (wake word / commands). No non-linear noise suppression.
    #[default]
    SpeechRecognition,
    /// Voice communication, 16 kHz input, with non-linear noise suppression.
    VoiceCommunication,
    /// Voice communication, 8 kHz input.
    VoiceCommunication8k,
    /// Full duplex (talk while playing), with non-linear noise suppression.
    FullDuplex,
}

impl AfeType {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::SpeechRecognition => 0,
            Self::VoiceCommunication => 1,
            Self::VoiceCommunication8k => 2,
            Self::FullDuplex => 3,
        }
    }
}

/// CPU budget of the AFE (`afe_mode_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AfeMode {
    /// Lower CPU and memory usage.
    #[default]
    LowCost,
    /// Better quality, more CPU and memory.
    HighPerformance,
}

impl AfeMode {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::LowCost => 0,
            Self::HighPerformance => 1,
        }
    }
}

/// Acoustic echo canceller algorithm (`aec_mode_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AecMode {
    /// Low cost AEC tuned for speech recognition.
    SrLowCost,
    /// High performance AEC tuned for speech recognition.
    SrHighPerf,
    /// Low cost AEC tuned for voice communication.
    VoipLowCost,
    /// High performance AEC tuned for voice communication.
    VoipHighPerf,
    /// Low cost AEC tuned for full duplex.
    FdLowCost,
    /// High performance AEC tuned for full duplex.
    FdHighPerf,
}

impl AecMode {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::SrLowCost => 0,
            Self::SrHighPerf => 1,
            Self::VoipLowCost => 3,
            Self::VoipHighPerf => 4,
            Self::FdLowCost => 5,
            Self::FdHighPerf => 6,
        }
    }
}

/// Strength of the AEC non-linear processing stage (`aec_nlp_level_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AecNlpLevel {
    /// Suitable for most scenarios.
    Normal,
    /// Stronger echo suppression (ESP-SR default).
    Aggressive,
    /// Strongest echo suppression.
    VeryAggressive,
}

impl AecNlpLevel {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Aggressive => 1,
            Self::VeryAggressive => 2,
        }
    }
}

/// Voice activity detector aggressiveness (`vad_mode_t`). Higher = fewer false positives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VadMode {
    /// Normal.
    Mode0,
    /// Aggressive.
    Mode1,
    /// Very aggressive.
    Mode2,
    /// Very very aggressive.
    Mode3,
    /// Very very very aggressive.
    Mode4,
}

impl VadMode {
    pub(crate) fn raw(self) -> u32 {
        self as u32
    }
}

/// WakeNet detection mode (`det_mode_t`), single channel.
///
/// Higher modes detect more readily but also raise the false alarm rate.
/// ESP-SR documents `Normal` (`DET_MODE_90`) as WakeNet10-only; WakeNet9 models
/// use `Aggressive` (`DET_MODE_95`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetMode {
    /// `DET_MODE_90`: lower false alarm rate (WakeNet10).
    Normal,
    /// `DET_MODE_95`: higher detection rate (default, works with WakeNet9).
    #[default]
    Aggressive,
}

impl DetMode {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::Normal => 0,
            Self::Aggressive => 1,
        }
    }
}

/// AGC algorithm (`afe_agc_mode_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgcMode {
    /// WebRTC AGC.
    WebRtc,
    /// Gain computed by the WakeNet model (only when WakeNet is enabled).
    WakeNet,
}

/// Where the AFE allocates its buffers (`afe_memory_alloc_mode_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemoryAlloc {
    /// Prefer internal RAM.
    MoreInternal,
    /// Balance internal RAM and PSRAM.
    Balanced,
    /// Prefer PSRAM (recommended on boards with PSRAM).
    #[default]
    MorePsram,
}

impl MemoryAlloc {
    pub(crate) fn raw(self) -> u32 {
        match self {
            Self::MoreInternal => 1,
            Self::Balanced => 2,
            Self::MorePsram => 3,
        }
    }
}

/// Voice activity detection settings.
#[derive(Debug, Clone, PartialEq)]
pub struct VadConfig {
    /// Aggressiveness.
    pub mode: VadMode,
    /// Minimum speech duration before reporting speech (ms, > 32, ESP-SR default 128).
    pub min_speech_ms: u32,
    /// Minimum silence duration before reporting silence (ms, > 64, ESP-SR default 1000).
    pub min_noise_ms: u32,
    /// VADNet model name (e.g. `"vadnet1_medium"`), `None` = WebRTC VAD.
    /// Use [`crate::Models::find`] to look one up in the model partition.
    pub model: Option<String>,
}

impl Default for VadConfig {
    fn default() -> Self {
        Self {
            mode: VadMode::Mode0,
            min_speech_ms: 128,
            min_noise_ms: 1000,
            model: None,
        }
    }
}

/// AEC settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AecConfig {
    /// Algorithm.
    pub mode: AecMode,
    /// Non-linear processing level.
    pub nlp_level: AecNlpLevel,
    /// Adaptive filter length (`None` = ESP-SR default for the chip).
    pub filter_length: Option<u32>,
}

/// AGC settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AgcConfig {
    /// Algorithm.
    pub mode: AgcMode,
    /// Compression gain in dB (ESP-SR default 9).
    pub compression_gain_db: i32,
    /// Target level in -dBFS (ESP-SR default 3).
    pub target_level_dbfs: i32,
}

impl Default for AgcConfig {
    fn default() -> Self {
        Self {
            mode: AgcMode::WebRtc,
            compression_gain_db: 9,
            target_level_dbfs: 3,
        }
    }
}

/// Setting that is either left to ESP-SR's default for the input format,
/// forced on with a configuration, or forced off.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Toggle<T> {
    /// Keep what `afe_config_init()` chose for this chip / input format.
    #[default]
    Default,
    /// Enable with this configuration.
    On(T),
    /// Disable.
    Off,
}

/// Builder for an [`crate::Afe`] configuration.
///
/// Starts from ESP-SR's own defaults (`afe_config_init()`), which enable as many
/// algorithms as the chip and the input format allow, then applies the overrides.
///
/// ```ignore
/// let cfg = AfeConfig::new("MMR")?
///     .afe_type(AfeType::SpeechRecognition)
///     .aec(AecConfig { mode: AecMode::SrLowCost, nlp_level: AecNlpLevel::Aggressive, filter_length: None })
///     .ns(true)
///     .vad(VadConfig { min_noise_ms: 500, ..Default::default() })
///     .wakenet("wn9_jarvis_tts");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AfeConfig {
    pub(crate) input_format: String,
    pub(crate) afe_type: AfeType,
    pub(crate) mode: AfeMode,
    pub(crate) aec: Toggle<AecConfig>,
    pub(crate) se: Option<bool>,
    pub(crate) ns: Option<bool>,
    pub(crate) vad: Toggle<VadConfig>,
    pub(crate) agc: Toggle<AgcConfig>,
    pub(crate) wakenet: Toggle<(String, Option<String>)>,
    pub(crate) wakenet_mode: Option<DetMode>,
    pub(crate) memory: MemoryAlloc,
    pub(crate) linear_gain: Option<f32>,
    pub(crate) core: Option<i32>,
    pub(crate) priority: Option<i32>,
    pub(crate) ringbuf_size: Option<i32>,
    pub(crate) vad_energy_threshold: Option<f32>,
}

impl AfeConfig {
    /// Create a configuration for the given channel layout of the data passed to
    /// [`crate::Afe::feed`].
    ///
    /// One character per interleaved channel: `M` = microphone, `R` = playback
    /// reference (for AEC), `N` = unused. Examples: `"M"`, `"MR"`, `"MMR"`, `"MMNR"`.
    pub fn new(input_format: &str) -> Result<Self, Error> {
        let valid = !input_format.is_empty()
            && input_format.len() <= 8
            && input_format.chars().all(|c| matches!(c, 'M' | 'R' | 'N'))
            && input_format.contains('M');
        if !valid {
            return Err(Error::InvalidInputFormat(input_format.to_owned()));
        }
        Ok(Self {
            input_format: input_format.to_owned(),
            afe_type: AfeType::default(),
            mode: AfeMode::default(),
            aec: Toggle::Default,
            se: None,
            ns: None,
            vad: Toggle::Default,
            agc: Toggle::Default,
            wakenet: Toggle::Default,
            wakenet_mode: None,
            memory: MemoryAlloc::default(),
            linear_gain: None,
            core: None,
            priority: None,
            ringbuf_size: None,
            vad_energy_threshold: None,
        })
    }

    /// Input format string given to [`AfeConfig::new`].
    pub fn input_format(&self) -> &str {
        &self.input_format
    }

    /// Number of interleaved channels expected by `feed()`.
    pub fn total_channels(&self) -> usize {
        self.input_format.len()
    }

    /// Scenario (default: speech recognition).
    pub fn afe_type(mut self, t: AfeType) -> Self {
        self.afe_type = t;
        self
    }

    /// CPU budget (default: low cost).
    pub fn mode(mut self, m: AfeMode) -> Self {
        self.mode = m;
        self
    }

    /// Enable AEC with the given settings (needs an `R` channel).
    pub fn aec(mut self, aec: AecConfig) -> Self {
        self.aec = Toggle::On(aec);
        self
    }

    /// Disable AEC.
    pub fn no_aec(mut self) -> Self {
        self.aec = Toggle::Off;
        self
    }

    /// Enable/disable speech enhancement (BSS / beamforming, needs 2+ mics).
    pub fn se(mut self, on: bool) -> Self {
        self.se = Some(on);
        self
    }

    /// Enable/disable noise suppression (WebRTC NS).
    pub fn ns(mut self, on: bool) -> Self {
        self.ns = Some(on);
        self
    }

    /// Enable VAD with the given settings.
    pub fn vad(mut self, vad: VadConfig) -> Self {
        self.vad = Toggle::On(vad);
        self
    }

    /// Disable VAD.
    pub fn no_vad(mut self) -> Self {
        self.vad = Toggle::Off;
        self
    }

    /// Minimum averaged frame energy (dBFS) for VADNet to report speech.
    pub fn vad_energy_threshold(mut self, dbfs: f32) -> Self {
        self.vad_energy_threshold = Some(dbfs);
        self
    }

    /// Enable AGC.
    pub fn agc(mut self, agc: AgcConfig) -> Self {
        self.agc = Toggle::On(agc);
        self
    }

    /// Disable AGC.
    pub fn no_agc(mut self) -> Self {
        self.agc = Toggle::Off;
        self
    }

    /// Enable WakeNet with one model (name as listed by [`crate::Models::names`],
    /// e.g. `"wn9_hiesp"`).
    pub fn wakenet(mut self, model: &str) -> Self {
        self.wakenet = Toggle::On((model.to_owned(), None));
        self
    }

    /// Enable WakeNet with two models running in parallel.
    pub fn wakenet2(mut self, model1: &str, model2: &str) -> Self {
        self.wakenet = Toggle::On((model1.to_owned(), Some(model2.to_owned())));
        self
    }

    /// WakeNet detection mode (`wakenet_mode`), default: ESP-SR's choice.
    pub fn wakenet_mode(mut self, mode: DetMode) -> Self {
        self.wakenet_mode = Some(mode);
        self
    }

    /// WakeNet model names configured with [`AfeConfig::wakenet`] / [`AfeConfig::wakenet2`].
    pub fn wakenet_models(&self) -> Vec<&str> {
        match &self.wakenet {
            Toggle::On((a, b)) => core::iter::once(a.as_str()).chain(b.as_deref()).collect(),
            _ => Vec::new(),
        }
    }

    /// Disable WakeNet.
    pub fn no_wakenet(mut self) -> Self {
        self.wakenet = Toggle::Off;
        self
    }

    /// Memory allocation strategy (default: prefer PSRAM).
    pub fn memory(mut self, m: MemoryAlloc) -> Self {
        self.memory = m;
        self
    }

    /// Linear output gain, 0.1 ..= 10.0.
    pub fn linear_gain(mut self, gain: f32) -> Self {
        self.linear_gain = Some(gain.clamp(0.1, 10.0));
        self
    }

    /// CPU core and FreeRTOS priority of the internal AFE task.
    pub fn task(mut self, core: i32, priority: i32) -> Self {
        self.core = Some(core);
        self.priority = Some(priority);
        self
    }

    /// Size of the internal ring buffer, in frames.
    pub fn ringbuf_frames(mut self, frames: i32) -> Self {
        self.ringbuf_size = Some(frames);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_format_validation() {
        assert!(AfeConfig::new("MMR").is_ok());
        assert_eq!(AfeConfig::new("MMR").unwrap().total_channels(), 3);
        assert!(AfeConfig::new("").is_err());
        assert!(AfeConfig::new("RR").is_err());
        assert!(AfeConfig::new("MX").is_err());
    }

    #[test]
    fn builder() {
        let c = AfeConfig::new("MR")
            .unwrap()
            .wakenet2("wn9_a", "wn9_b")
            .linear_gain(42.0);
        assert_eq!(c.wakenet_models(), vec!["wn9_a", "wn9_b"]);
        assert_eq!(c.linear_gain, Some(10.0));
        assert!(c.no_wakenet().wakenet_models().is_empty());
    }
}
