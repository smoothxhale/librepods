//! Publishes the latest battery levels of a connected device as a small JSON file
//! under `$XDG_RUNTIME_DIR/librepods/`, so status bars can show them without a tray.

use std::path::PathBuf;

use log::warn;
use serde_json::{Value, json};

use crate::bluetooth::aacp::{BatteryComponent, BatteryInfo, BatteryStatus};

/// Directory holding one `<MAC>.json` file per connected device.
fn state_dir() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime).join("librepods")
}

fn state_path(mac: &str) -> PathBuf {
    state_dir().join(format!("{}.json", mac.replace(':', "_")))
}

/// JSON document for one battery report. Components the device reports as
/// disconnected (for example a bud that is not in range) are `null`.
fn render(info: &[BatteryInfo]) -> Value {
    let component = |which: BatteryComponent| -> Value {
        info.iter()
            .find(|b| b.component == which && b.status != BatteryStatus::Disconnected)
            .map_or(Value::Null, |b| {
                json!({
                    "level": b.level,
                    "charging": b.status == BatteryStatus::Charging,
                })
            })
    };
    json!({
        "left": component(BatteryComponent::Left),
        "right": component(BatteryComponent::Right),
        "case": component(BatteryComponent::Case),
        "headphone": component(BatteryComponent::Headphone),
    })
}

/// Write the battery state for `mac`. The file is replaced atomically, so readers
/// never observe a partially written document.
pub fn publish(mac: &str, info: &[BatteryInfo]) {
    let dir = state_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("could not create battery state dir {}: {}", dir.display(), e);
        return;
    }
    let path = state_path(mac);
    let tmp = path.with_extension("json.tmp");
    let body = render(info).to_string();
    if let Err(e) = std::fs::write(&tmp, body).and_then(|_| std::fs::rename(&tmp, &path)) {
        warn!("could not write battery state {}: {}", path.display(), e);
    }
}

/// Remove the battery state for `mac` once the device disconnects.
pub fn clear(mac: &str) {
    let path = state_path(mac);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        warn!("could not remove battery state {}: {}", path.display(), e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn battery(component: BatteryComponent, level: u8, status: BatteryStatus) -> BatteryInfo {
        BatteryInfo {
            component,
            level,
            status,
        }
    }

    #[test]
    fn reports_levels_and_charging_per_component() {
        let doc = render(&[
            battery(BatteryComponent::Right, 62, BatteryStatus::NotCharging),
            battery(BatteryComponent::Left, 8, BatteryStatus::Charging),
            battery(BatteryComponent::Case, 40, BatteryStatus::NotCharging),
        ]);
        assert_eq!(doc["right"], json!({"level": 62, "charging": false}));
        assert_eq!(doc["left"], json!({"level": 8, "charging": true}));
        assert_eq!(doc["case"], json!({"level": 40, "charging": false}));
        assert_eq!(doc["headphone"], Value::Null);
    }

    #[test]
    fn disconnected_component_is_null() {
        let doc = render(&[
            battery(BatteryComponent::Right, 63, BatteryStatus::NotCharging),
            battery(BatteryComponent::Left, 0, BatteryStatus::Disconnected),
        ]);
        assert_eq!(doc["left"], Value::Null);
        assert_eq!(doc["right"]["level"], 63);
    }

    #[test]
    fn state_path_uses_underscored_mac() {
        let path = state_path("EC:E9:D2:3A:3A:5B");
        assert!(path.ends_with("librepods/EC_E9_D2_3A_3A_5B.json"));
    }
}
