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

use crate::bluetooth::aacp::ControlCommandStatus;
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
        ["set", name, value] => set(managers, name, value).await,
        _ => error("usage: status | settings | set <setting> <value>"),
    }
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
        let reported = aacp.state.lock().await.control_command_status_list.clone();
        let settings: Map<String, Value> = Setting::ALL
            .into_iter()
            .filter_map(|setting| {
                reported
                    .iter()
                    .find(|r| r.identifier == setting.identifier())
                    .map(|r| (setting.name().to_string(), json!(setting.describe(&r.value))))
            })
            .collect();
        devices.push(json!({ "mac": mac, "settings": settings }));
    }
    json!({ "ok": true, "devices": devices })
}

fn settings_help() -> Value {
    let settings: Map<String, Value> = Setting::ALL
        .into_iter()
        .map(|s| (s.name().to_string(), json!(s.accepted())))
        .collect();
    json!({ "ok": true, "settings": settings })
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
    }
}
