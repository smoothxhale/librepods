//! AirPods settings carried as AACP control commands: user-facing names, value
//! parsing for writes, and readable values for status. Settings whose value
//! encoding is not documented are reported but cannot be written.

use crate::bluetooth::aacp::ControlCommandIdentifiers as Id;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    MicMode,
    EarDetection,
    PressSpeed,
    PressHold,
    ChimeVolume,
    InCaseTone,
    SleepDetection,
    AdaptiveVolume,
    CallManagement,
    UplinkEq,
}

enum Kind {
    /// One byte chosen from named values.
    Choice(&'static [(&'static str, u8)]),
    /// One byte, 0 to 100.
    Percent,
    /// Reported by the device; encoding unknown, so never written.
    ReadOnly,
}

/// Boolean words accepted wherever a setting takes on/off.
pub fn parse_toggle(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "enable" | "enabled" => Some(true),
        "off" | "false" | "no" | "0" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

const MIC_MODES: &[(&str, u8)] = &[("auto", 0x00), ("right", 0x01), ("left", 0x02)];
const INTERVALS: &[(&str, u8)] = &[("default", 0x00), ("slower", 0x01), ("slowest", 0x02)];
const TOGGLE: &[(&str, u8)] = &[("on", 0x01), ("off", 0x02)];

impl Setting {
    pub const ALL: [Setting; 10] = [
        Setting::MicMode,
        Setting::EarDetection,
        Setting::PressSpeed,
        Setting::PressHold,
        Setting::ChimeVolume,
        Setting::InCaseTone,
        Setting::SleepDetection,
        Setting::AdaptiveVolume,
        Setting::CallManagement,
        Setting::UplinkEq,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Setting::MicMode => "mic-mode",
            Setting::EarDetection => "ear-detection",
            Setting::PressSpeed => "press-speed",
            Setting::PressHold => "press-hold",
            Setting::ChimeVolume => "chime-volume",
            Setting::InCaseTone => "in-case-tone",
            Setting::SleepDetection => "sleep-detection",
            Setting::AdaptiveVolume => "adaptive-volume",
            Setting::CallManagement => "call-management",
            Setting::UplinkEq => "uplink-eq",
        }
    }

    pub fn identifier(self) -> Id {
        match self {
            Setting::MicMode => Id::MicMode,
            Setting::EarDetection => Id::EarDetectionConfig,
            Setting::PressSpeed => Id::DoubleClickInterval,
            Setting::PressHold => Id::ClickHoldInterval,
            Setting::ChimeVolume => Id::ChimeVolume,
            Setting::InCaseTone => Id::InCaseToneConfig,
            Setting::SleepDetection => Id::SleepDetectionConfig,
            Setting::AdaptiveVolume => Id::AdaptiveVolumeConfig,
            Setting::CallManagement => Id::CallManagementConfig,
            Setting::UplinkEq => Id::UplinkEqBudConfig,
        }
    }

    fn kind(self) -> Kind {
        match self {
            Setting::MicMode => Kind::Choice(MIC_MODES),
            Setting::PressSpeed | Setting::PressHold => Kind::Choice(INTERVALS),
            Setting::EarDetection
            | Setting::InCaseTone
            | Setting::SleepDetection
            | Setting::AdaptiveVolume => Kind::Choice(TOGGLE),
            Setting::ChimeVolume => Kind::Percent,
            Setting::CallManagement | Setting::UplinkEq => Kind::ReadOnly,
        }
    }

    pub fn from_name(name: &str) -> Option<Setting> {
        Setting::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Accepted values, for help output.
    pub fn accepted(self) -> String {
        match self.kind() {
            Kind::Choice(options) => options
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join("|"),
            Kind::Percent => "0..100".to_string(),
            Kind::ReadOnly => "read-only".to_string(),
        }
    }

    /// Control-command payload for a user-supplied value.
    pub fn encode(self, value: &str) -> Result<Vec<u8>, String> {
        match self.kind() {
            Kind::Choice(options) => {
                // Toggles also accept true/false, yes/no, 1/0 and enable/disable.
                let word = match (std::ptr::eq(options, TOGGLE), parse_toggle(value)) {
                    (true, Some(true)) => "on",
                    (true, Some(false)) => "off",
                    _ => value,
                };
                options
                    .iter()
                    .find(|(name, _)| *name == word)
                    .map(|(_, byte)| vec![*byte])
                    .ok_or_else(|| format!("{}: expected {}", self.name(), self.accepted()))
            }
            Kind::Percent => value
                .parse::<u8>()
                .ok()
                .filter(|v| *v <= 100)
                .map(|v| vec![v])
                .ok_or_else(|| format!("{}: expected 0..100", self.name())),
            Kind::ReadOnly => Err(format!(
                "{} is read-only: its value encoding is not documented",
                self.name()
            )),
        }
    }

    /// Readable form of a value reported by the device.
    pub fn describe(self, raw: &[u8]) -> String {
        let hex = || format!("0x{}", hex::encode(raw));
        match (self.kind(), raw.first().copied()) {
            (Kind::Choice(options), Some(byte)) => options
                .iter()
                .find(|(_, v)| *v == byte)
                .map_or_else(hex, |(name, _)| name.to_string()),
            (Kind::Percent, Some(byte)) => byte.to_string(),
            _ => hex(),
        }
    }

    fn about(self) -> &'static str {
        match self {
            Setting::MicMode => "which bud's microphone is used",
            Setting::EarDetection => "react to taking buds out (pause, switch output)",
            Setting::PressSpeed => "how fast stem presses must be",
            Setting::PressHold => "how long a press-and-hold lasts",
            Setting::ChimeVolume => "volume of connect and low-battery chimes",
            Setting::InCaseTone => "tone when the buds go into the case",
            Setting::SleepDetection => "pause media when you fall asleep",
            Setting::AdaptiveVolume => "adapt volume to your surroundings",
            Setting::CallManagement => "call controls (encoding unknown)",
            Setting::UplinkEq => "microphone EQ per bud (encoding unknown)",
        }
    }
}

/// LibrePods' own settings for the hi-res microphone (stored in app_settings.json).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSetting {
    HiresMic,
    Agc,
    AgcTarget,
    AgcMaxBoost,
    AgcGate,
    AgcAttack,
    AgcRelease,
}

impl AppSetting {
    pub const ALL: [AppSetting; 7] = [
        AppSetting::HiresMic,
        AppSetting::Agc,
        AppSetting::AgcTarget,
        AppSetting::AgcMaxBoost,
        AppSetting::AgcGate,
        AppSetting::AgcAttack,
        AppSetting::AgcRelease,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AppSetting::HiresMic => "hires-mic",
            AppSetting::Agc => "agc",
            AppSetting::AgcTarget => "agc-target",
            AppSetting::AgcMaxBoost => "agc-max-boost",
            AppSetting::AgcGate => "agc-gate",
            AppSetting::AgcAttack => "agc-attack",
            AppSetting::AgcRelease => "agc-release",
        }
    }

    pub fn from_name(name: &str) -> Option<AppSetting> {
        AppSetting::ALL.into_iter().find(|s| s.name() == name)
    }

    /// Inclusive numeric range, or None for an on/off setting.
    pub fn range(self) -> Option<(f32, f32)> {
        match self {
            AppSetting::HiresMic | AppSetting::Agc => None,
            AppSetting::AgcTarget => Some((-30.0, -3.0)),
            AppSetting::AgcMaxBoost => Some((0.0, 30.0)),
            AppSetting::AgcGate => Some((-80.0, -20.0)),
            AppSetting::AgcAttack => Some((1.0, 100.0)),
            AppSetting::AgcRelease => Some((50.0, 3000.0)),
        }
    }

    fn about(self) -> &'static str {
        match self {
            AppSetting::HiresMic => "high-quality AirPods microphone (\"AirPodsHiRes\" input)",
            AppSetting::Agc => "automatic gain for the hi-res microphone",
            AppSetting::AgcTarget => "speech level the gain aims for, dBFS",
            AppSetting::AgcMaxBoost => "largest boost applied, dB",
            AppSetting::AgcGate => "below this level input counts as background, dBFS",
            AppSetting::AgcAttack => "how fast gain drops when you get loud, ms",
            AppSetting::AgcRelease => "how fast gain recovers after you get quiet, ms",
        }
    }
}

/// A setting as seen by users and scripts: name, accepted values and a short
/// description.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSpec {
    pub name: &'static str,
    pub accepted: String,
    pub about: &'static str,
    pub writable: bool,
}

const EQ_BANDS: [(&str, &str); 3] = [
    ("eq-low", "bass gain, 50 = flat"),
    ("eq-mid", "mid gain, 50 = flat"),
    ("eq-high", "treble gain, 50 = flat"),
];

/// Every setting `get`, `set`, `export` and `apply` understand.
pub fn all_settings() -> Vec<SettingSpec> {
    let mut specs: Vec<SettingSpec> = Setting::ALL
        .into_iter()
        .map(|s| SettingSpec {
            name: s.name(),
            accepted: s.accepted(),
            about: s.about(),
            writable: !matches!(s.kind(), Kind::ReadOnly),
        })
        .collect();
    specs.push(SettingSpec {
        name: "eq",
        accepted: "on|off".to_string(),
        about: "custom EQ",
        writable: true,
    });
    for (name, about) in EQ_BANDS {
        specs.push(SettingSpec {
            name,
            accepted: format!("0..{}", crate::bluetooth::aacp_extras::EQ_BAND_MAX),
            about,
            writable: true,
        });
    }
    for s in AppSetting::ALL {
        specs.push(SettingSpec {
            name: s.name(),
            accepted: match s.range() {
                Some((lo, hi)) => format!("{lo}..{hi}"),
                None => "on|off".to_string(),
            },
            about: s.about(),
            writable: true,
        });
    }
    specs
}

/// Check a value for any setting and return it in canonical form (toggles as
/// on/off, numbers as written). Errors name the accepted values.
pub fn validate(name: &str, value: &str) -> Result<String, String> {
    let spec = all_settings()
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| format!("unknown setting '{name}'"))?;
    if !spec.writable {
        return Err(format!("{name} is read-only"));
    }
    if spec.accepted == "on|off" {
        return parse_toggle(value)
            .map(|on| if on { "on" } else { "off" }.to_string())
            .ok_or_else(|| format!("{name}: expected on|off, got '{value}'"));
    }
    if let Some((lo, hi)) = spec.accepted.split_once("..") {
        let (lo, hi): (f32, f32) = (
            lo.parse().unwrap_or(f32::MIN),
            hi.parse().unwrap_or(f32::MAX),
        );
        return value
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v >= lo && *v <= hi)
            .map(|_| value.to_string())
            .ok_or_else(|| {
                format!(
                    "{name}: expected a number in {}, got '{value}'",
                    spec.accepted
                )
            });
    }
    if spec.accepted.split('|').any(|choice| choice == value) {
        Ok(value.to_string())
    } else {
        Err(format!("{name}: expected {}, got '{value}'", spec.accepted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for setting in Setting::ALL {
            assert_eq!(Setting::from_name(setting.name()), Some(setting));
        }
        assert_eq!(Setting::from_name("volume"), None);
    }

    #[test]
    fn encodes_documented_values() {
        assert_eq!(Setting::MicMode.encode("left"), Ok(vec![0x02]));
        assert_eq!(Setting::EarDetection.encode("off"), Ok(vec![0x02]));
        assert_eq!(Setting::PressSpeed.encode("slowest"), Ok(vec![0x02]));
        assert_eq!(Setting::ChimeVolume.encode("64"), Ok(vec![64]));
    }

    #[test]
    fn spec_covers_device_eq_and_app_settings() {
        let names: Vec<&str> = all_settings().iter().map(|s| s.name).collect();
        for expected in [
            "press-speed",
            "eq",
            "eq-mid",
            "hires-mic",
            "agc-target",
            "uplink-eq",
        ] {
            assert!(names.contains(&expected), "{expected}");
        }
        let target = all_settings()
            .into_iter()
            .find(|s| s.name == "agc-target")
            .unwrap();
        assert_eq!(target.accepted, "-30..-3");
    }

    #[test]
    fn validate_normalises_and_explains() {
        assert_eq!(validate("agc", "yes"), Ok("on".to_string()));
        assert_eq!(validate("eq", "0"), Ok("off".to_string()));
        assert_eq!(validate("agc-target", "-14"), Ok("-14".to_string()));
        assert_eq!(
            validate("press-speed", "slowest"),
            Ok("slowest".to_string())
        );
        assert!(
            validate("agc-target", "-2")
                .unwrap_err()
                .contains("-30..-3")
        );
        assert!(validate("eq-low", "101").unwrap_err().contains("0..100"));
        assert!(
            validate("mic-mode", "middle")
                .unwrap_err()
                .contains("auto|right|left")
        );
        assert!(
            validate("uplink-eq", "1")
                .unwrap_err()
                .contains("read-only")
        );
        assert!(
            validate("volume", "5")
                .unwrap_err()
                .contains("unknown setting")
        );
    }

    #[test]
    fn toggles_accept_boolean_words() {
        for word in ["on", "true", "yes", "1", "Enable"] {
            assert_eq!(
                Setting::SleepDetection.encode(word),
                Ok(vec![0x01]),
                "{word}"
            );
        }
        for word in ["off", "FALSE", "no", "0", "disabled"] {
            assert_eq!(
                Setting::SleepDetection.encode(word),
                Ok(vec![0x02]),
                "{word}"
            );
        }
        // Non-toggle choices keep their exact words.
        assert!(Setting::MicMode.encode("yes").is_err());
    }

    #[test]
    fn rejects_invalid_values() {
        assert!(Setting::MicMode.encode("middle").is_err());
        assert!(Setting::ChimeVolume.encode("101").is_err());
        assert!(Setting::ChimeVolume.encode("-1").is_err());
        assert!(Setting::CallManagement.encode("1").is_err());
    }

    #[test]
    fn describes_reported_values() {
        assert_eq!(Setting::PressHold.describe(&[0x00]), "default");
        assert_eq!(Setting::SleepDetection.describe(&[0x02]), "off");
        assert_eq!(Setting::ChimeVolume.describe(&[0x40, 0x40]), "64");
        assert_eq!(Setting::CallManagement.describe(&[0x00, 0x03]), "0x0003");
        assert_eq!(Setting::MicMode.describe(&[0x09]), "0x09");
    }
}
