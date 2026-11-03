//! Local control socket for the running instance. Each connection sends one
//! request line and receives one JSON reply:
//!   `status`                    connected devices and their reported settings
//!   `settings`                  setting names and accepted values
//!   `set <setting> <value>`     change a setting on every connected device

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use log::{info, warn};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::RwLock;

use crate::bluetooth::aacp::{AACPManager, ControlCommandStatus};
use crate::bluetooth::aacp_extras::{CustomEq, EQ_BAND_MAX, NAME_MAX_BYTES, validate_name};
use crate::bluetooth::managers::DeviceManagers;
use crate::control::settings::Setting;

/// Longest request line accepted; requests are a few words.
const MAX_REQUEST_BYTES: u64 = 4096;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

type Managers = Arc<RwLock<HashMap<String, DeviceManagers>>>;

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(runtime).join("librepods").join("control.sock")
}

/// Accept control connections until the process exits.
pub async fn serve(managers: Managers) {
    use std::os::unix::fs::PermissionsExt;

    let path = socket_path();
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        warn!("control socket: cannot create {}: {}", dir.display(), e);
        return;
    }
    // A socket file left by a previous run would make bind fail.
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(listener) => listener,
        Err(e) => {
            warn!("control socket: cannot bind {}: {}", path.display(), e);
            return;
        }
    };
    if let Err(e) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
        warn!("control socket: cannot restrict permissions: {}", e);
    }
    info!("Control socket listening on {}", path.display());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let managers = managers.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, managers).await {
                        warn!("control socket: request failed: {}", e);
                    }
                });
            }
            Err(e) => {
                warn!("control socket: accept failed: {}", e);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn handle(stream: UnixStream, managers: Managers) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read.take(MAX_REQUEST_BYTES))
        .read_line(&mut line)
        .await?;
    let reply = respond(line.trim(), &managers).await;
    write.write_all(format!("{reply}\n").as_bytes()).await
}

async fn respond(request: &str, managers: &Managers) -> Value {
    let words: Vec<&str> = request.split_whitespace().collect();
    match words.as_slice() {
        ["status"] => status(managers).await,
        ["settings"] => settings_help(),
        ["set", name, value] if name.starts_with("eq") => set_eq(managers, name, value).await,
        ["set", name, value] => set(managers, name, value).await,
        ["head-tracking", state] => head_tracking(managers, state).await,
        ["rename", name @ ..] if !name.is_empty() => rename(managers, &name.join(" ")).await,
        _ => error(
            "usage: status | settings | set <setting> <value> | head-tracking on|off | rename <name>",
        ),
    }
}

/// Run `action` on every connected device's AACP manager, returning the MACs it
/// succeeded on, or an error when none did.
async fn for_each_device<F, Fut>(managers: &Managers, what: &str, action: F) -> Result<Vec<String>, String>
where
    F: Fn(Arc<AACPManager>) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let managers = managers.read().await;
    let mut applied = Vec::new();
    let mut last_error = None;
    for (mac, manager) in managers.iter() {
        let Some(aacp) = manager.get_aacp() else {
            continue;
        };
        match action(aacp).await {
            Ok(()) => {
                info!("control socket: {what} on {mac}");
                applied.push(mac.clone());
            }
            Err(e) => {
                warn!("control socket: {what} on {mac} failed: {e}");
                last_error = Some(e);
            }
        }
    }
    if applied.is_empty() {
        return Err(last_error.unwrap_or_else(|| "no connected AirPods".to_string()));
    }
    Ok(applied)
}

/// New custom EQ after applying `eq on|off` or `eq-low|eq-mid|eq-high 0-100`.
fn apply_eq(current: CustomEq, name: &str, value: &str) -> Result<CustomEq, String> {
    let band = || {
        value
            .parse::<u8>()
            .ok()
            .filter(|v| *v <= EQ_BAND_MAX)
            .ok_or_else(|| format!("{name}: expected 0-{EQ_BAND_MAX}"))
    };
    Ok(match name {
        "eq" => match value {
            "on" => CustomEq { enabled: true, ..current },
            "off" => CustomEq { enabled: false, ..current },
            _ => return Err("eq: expected on|off".to_string()),
        },
        "eq-low" => CustomEq { low: band()?, ..current },
        "eq-mid" => CustomEq { mid: band()?, ..current },
        "eq-high" => CustomEq { high: band()?, ..current },
        _ => return Err(format!("unknown setting '{name}'; run 'settings' for the list")),
    })
}

async fn set_eq(managers: &Managers, name: &str, value: &str) -> Value {
    // Validate against a neutral EQ first so bad input never reaches a device.
    let neutral = CustomEq { enabled: true, low: 50, mid: 50, high: 50 };
    if let Err(e) = apply_eq(neutral, name, value) {
        return error(e);
    }
    let result = for_each_device(managers, &format!("set {name} = {value}"), |aacp| async move {
        let current = aacp
            .state
            .lock()
            .await
            .custom_eq
            .ok_or("device has not reported a custom EQ")?;
        let updated = apply_eq(current, name, value)?;
        aacp.send_custom_eq(updated).await.map_err(|e| e.to_string())
    })
    .await;
    match result {
        Ok(applied) => json!({ "ok": true, "setting": name, "value": value, "applied": applied }),
        Err(e) => error(e),
    }
}

async fn head_tracking(managers: &Managers, state: &str) -> Value {
    let start = match state {
        "on" => true,
        "off" => false,
        _ => return error("head-tracking: expected on|off"),
    };
    let result = for_each_device(managers, &format!("head-tracking {state}"), |aacp| async move {
        let sent = if start {
            aacp.start_head_tracking().await
        } else {
            aacp.stop_head_tracking().await
        };
        sent.map_err(|e| e.to_string())
    })
    .await;
    match result {
        Ok(applied) => json!({ "ok": true, "head-tracking": state, "applied": applied }),
        Err(e) => error(e),
    }
}

async fn rename(managers: &Managers, name: &str) -> Value {
    let name = match validate_name(name) {
        Ok(name) => name.to_string(),
        Err(e) => return error(e),
    };
    let result = for_each_device(managers, &format!("rename to '{name}'"), |aacp| {
        let name = name.clone();
        async move { aacp.send_rename_packet(&name).await.map_err(|e| e.to_string()) }
    })
    .await;
    match result {
        Ok(applied) => json!({ "ok": true, "name": name, "applied": applied }),
        Err(e) => error(e),
    }
}

/// Angle rounded to one decimal, computed in f64 so JSON shows e.g. 2.4, not
/// the nearest f32 (2.4000000953674316).
fn tenths(degrees: f32) -> f64 {
    (f64::from(degrees) * 10.0).round() / 10.0
}

fn error(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": message.into() })
}

async fn status(managers: &Managers) -> Value {
    let managers = managers.read().await;
    let mut devices = Vec::new();
    for (mac, manager) in managers.iter() {
        let Some(aacp) = manager.get_aacp() else {
            continue;
        };
        let (reported, custom_eq, head) = {
            let state = aacp.state.lock().await;
            let head = state.head_tracker.as_ref().map(|t| match t.pose() {
                Some(p) => json!({
                    "calibrating": false,
                    "yaw": tenths(p.yaw),
                    "pitch": tenths(p.pitch),
                    "roll": tenths(p.roll),
                }),
                None => json!({ "calibrating": t.calibrating() }),
            });
            (state.control_command_status_list.clone(), state.custom_eq, head)
        };
        let mut settings: Map<String, Value> = Setting::ALL
            .into_iter()
            .filter_map(|setting| {
                reported
                    .iter()
                    .find(|r| r.identifier == setting.identifier())
                    .map(|r| (setting.name().to_string(), json!(setting.describe(&r.value))))
            })
            .collect();
        if let Some(eq) = custom_eq {
            settings.insert("eq".to_string(), json!(if eq.enabled { "on" } else { "off" }));
            settings.insert("eq-low".to_string(), json!(eq.low.to_string()));
            settings.insert("eq-mid".to_string(), json!(eq.mid.to_string()));
            settings.insert("eq-high".to_string(), json!(eq.high.to_string()));
        }
        let mut device = json!({ "mac": mac, "settings": settings });
        if let Some(head) = head {
            device["head"] = head;
        }
        devices.push(device);
    }
    json!({ "ok": true, "devices": devices })
}

fn settings_help() -> Value {
    let mut settings: Map<String, Value> = Setting::ALL
        .into_iter()
        .map(|s| (s.name().to_string(), json!(s.accepted())))
        .collect();
    let band = format!("0-{EQ_BAND_MAX}");
    settings.insert("eq".to_string(), json!("on|off"));
    for name in ["eq-low", "eq-mid", "eq-high"] {
        settings.insert(name.to_string(), json!(band));
    }
    json!({
        "ok": true,
        "settings": settings,
        "commands": {
            "head-tracking": "on|off",
            "rename": format!("<name, up to {NAME_MAX_BYTES} bytes>"),
        },
    })
}

async fn set(managers: &Managers, name: &str, value: &str) -> Value {
    let Some(setting) = Setting::from_name(name) else {
        return error(format!("unknown setting '{name}'; run 'settings' for the list"));
    };
    let payload = match setting.encode(value) {
        Ok(payload) => payload,
        Err(e) => return error(e),
    };

    let managers = managers.read().await;
    let mut applied = Vec::new();
    for (mac, manager) in managers.iter() {
        let Some(aacp) = manager.get_aacp() else {
            continue;
        };
        match aacp
            .send_control_command(setting.identifier(), &payload)
            .await
        {
            Ok(()) => {
                info!("control socket: set {} = {} on {}", setting.name(), value, mac);
                // AirPods apply settings without echoing them back, so record the
                // new value here; the device reports it again on the next connect.
                let mut state = aacp.state.lock().await;
                let list = &mut state.control_command_status_list;
                match list.iter_mut().find(|s| s.identifier == setting.identifier()) {
                    Some(status) => status.value = payload.clone(),
                    None => list.push(ControlCommandStatus {
                        identifier: setting.identifier(),
                        value: payload.clone(),
                    }),
                }
                applied.push(mac.clone());
            }
            Err(e) => warn!("control socket: set {} on {} failed: {}", setting.name(), mac, e),
        }
    }
    if applied.is_empty() {
        return error("no connected AirPods accepted the command");
    }
    json!({ "ok": true, "setting": setting.name(), "value": value, "applied": applied })
}

/// Send one request to the running instance and return its reply line.
pub fn request(line: &str) -> Result<String, String> {
    let path = socket_path();
    let mut stream = std::os::unix::net::UnixStream::connect(&path).map_err(|e| {
        format!("cannot reach LibrePods at {} ({e}); is it running?", path.display())
    })?;
    stream
        .set_read_timeout(Some(CLIENT_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{line}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    stream
        .read_to_string(&mut reply)
        .map_err(|e| e.to_string())?;
    Ok(reply.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Managers {
        Arc::new(RwLock::new(HashMap::new()))
    }

    #[tokio::test]
    async fn rejects_unknown_requests() {
        let reply = respond("reboot", &empty()).await;
        assert_eq!(reply["ok"], false);
    }

    #[tokio::test]
    async fn validates_before_sending() {
        let reply = respond("set mic-mode middle", &empty()).await;
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("auto|right|left"));

        let reply = respond("set volume 5", &empty()).await;
        assert!(reply["error"].as_str().unwrap().contains("unknown setting"));
    }

    #[tokio::test]
    async fn valid_set_without_devices_reports_nothing_applied() {
        let reply = respond("set press-speed slower", &empty()).await;
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("no connected AirPods"));
    }

    #[tokio::test]
    async fn status_and_settings_succeed_without_devices() {
        assert_eq!(respond("status", &empty()).await["devices"], json!([]));
        let help = respond("settings", &empty()).await;
        assert_eq!(help["settings"]["chime-volume"], "0-100");
        assert_eq!(help["settings"]["eq-mid"], "0-100");
        assert_eq!(help["commands"]["head-tracking"], "on|off");
    }

    const REPORTED_EQ: CustomEq = CustomEq {
        enabled: true,
        low: 75,
        mid: 31,
        high: 74,
    };

    #[test]
    fn eq_changes_touch_only_the_named_field() {
        let low = apply_eq(REPORTED_EQ, "eq-low", "20").unwrap();
        assert_eq!(low, CustomEq { low: 20, ..REPORTED_EQ });
        let off = apply_eq(REPORTED_EQ, "eq", "off").unwrap();
        assert_eq!(off, CustomEq { enabled: false, ..REPORTED_EQ });
    }

    #[test]
    fn eq_changes_reject_bad_values() {
        assert!(apply_eq(REPORTED_EQ, "eq-high", "101").is_err());
        assert!(apply_eq(REPORTED_EQ, "eq", "maybe").is_err());
        assert!(apply_eq(REPORTED_EQ, "eq-bass", "10").is_err());
    }

    #[tokio::test]
    async fn new_commands_validate_before_sending() {
        let m = empty();
        assert!(respond("set eq-mid 500", &m).await["error"]
            .as_str()
            .unwrap()
            .contains("0-100"));
        assert!(respond("head-tracking sideways", &m).await["error"]
            .as_str()
            .unwrap()
            .contains("on|off"));
        let long = format!("rename {}", "x".repeat(NAME_MAX_BYTES + 1));
        assert!(respond(&long, &m).await["error"]
            .as_str()
            .unwrap()
            .contains("limit"));
        assert_eq!(respond("rename", &m).await["ok"], false);
    }

    #[tokio::test]
    async fn new_commands_without_devices_report_no_airpods() {
        let m = empty();
        for request in ["set eq on", "head-tracking on", "rename Pods"] {
            let reply = respond(request, &m).await;
            assert_eq!(reply["ok"], false, "{request}");
            assert!(reply["error"].as_str().unwrap().contains("no connected AirPods"));
        }
    }
}
