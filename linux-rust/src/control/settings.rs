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
            Kind::Percent => "0-100".to_string(),
            Kind::ReadOnly => "read-only".to_string(),
        }
    }

    /// Control-command payload for a user-supplied value.
    pub fn encode(self, value: &str) -> Result<Vec<u8>, String> {
        match self.kind() {
            Kind::Choice(options) => options
                .iter()
                .find(|(name, _)| *name == value)
                .map(|(_, byte)| vec![*byte])
                .ok_or_else(|| format!("{}: expected {}", self.name(), self.accepted())),
            Kind::Percent => value
                .parse::<u8>()
                .ok()
                .filter(|v| *v <= 100)
                .map(|v| vec![v])
                .ok_or_else(|| format!("{}: expected 0-100", self.name())),
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
