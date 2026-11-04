//! Human-readable report for `librepods info`.

use serde_json::Value;

/// Product name for an AirPods model number.
pub fn model_name(model_number: &str) -> Option<&'static str> {
    const MODELS: &[(&[&str], &str)] = &[
        (&["A1523", "A1722"], "AirPods (1st generation)"),
        (&["A2032", "A2031"], "AirPods (2nd generation)"),
        (&["A2565", "A2564"], "AirPods (3rd generation)"),
        (&["A3053", "A3050", "A3054"], "AirPods 4"),
        (&["A3056", "A3055", "A3057"], "AirPods 4 (ANC)"),
        (&["A2084", "A2083"], "AirPods Pro"),
        (&["A2931", "A2699", "A2698"], "AirPods Pro 2 (Lightning)"),
        (&["A3047", "A3048", "A3049"], "AirPods Pro 2 (USB-C)"),
        (&["A3063", "A3064", "A3065"], "AirPods Pro 3"),
    ];
    MODELS
        .iter()
        .find(|(numbers, _)| numbers.contains(&model_number))
        .map(|(_, name)| *name)
}

/// Active A2DP codec of the headset with this address, from PipeWire.
pub fn bluetooth_codec(mac: &str) -> Option<String> {
    let output = std::process::Command::new("pactl")
        .args(["list", "sinks"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let sink = format!("bluez_output.{}", mac.replace(':', "_"));
    let block = text.split("\nSink #").find(|b| b.contains(&sink))?;
    block
        .lines()
        .find_map(|l| l.trim().strip_prefix("api.bluez5.codec = "))
        .map(|c| c.trim_matches('"').to_uppercase())
}

fn text(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| !s.is_empty())
}

/// Number without a trailing ".0" when it is whole (-12 rather than -12.0).
fn num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => v.to_string(),
    }
}

fn row(out: &mut String, label: &str, value: impl AsRef<str>) {
    out.push_str(&format!("  {label:<14} {}\n", value.as_ref()));
}

fn battery_line(battery: &Value) -> String {
    let parts: Vec<String> = battery
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["status"] != "Disconnected")
        .map(|b| {
            let name = match b["component"].as_str() {
                Some("left") => "L",
                Some("right") => "R",
                Some("case") => "case",
                other => other.unwrap_or("?"),
            };
            let charging = if b["status"] == "Charging" { " (charging)" } else { "" };
            format!("{name} {}%{charging}", b["level"])
        })
        .collect();
    if parts.is_empty() { "unknown".to_string() } else { parts.join("  ") }
}

fn audio_line(audio: &Value, codec: Option<&str>) -> String {
    let owner = match audio["owner"].as_str() {
        Some("this-device") => "playing from this computer".to_string(),
        Some("other-device") => {
            let others = audio["connected"].as_array().map_or(String::new(), |a| {
                a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
            });
            format!("in use by another device ({others})")
        }
        Some("none") => "connected, idle".to_string(),
        _ => "unknown".to_string(),
    };
    match codec {
        Some(c) => format!("{owner} · A2DP {c}"),
        None => owner,
    }
}

fn mic_line(mic: &Value) -> String {
    if mic["enabled"] != true {
        return "hi-res off".to_string();
    }
    let activity = if mic["active"] == true {
        format!("live ({})", text(&mic["recorder"]).unwrap_or("recorder"))
    } else {
        "idle".to_string()
    };
    let agc = if mic["agc"] == true {
        let t = &mic["agc_tuning"];
        format!(
            "AGC on (target {} dBFS, boost {} dB, gate {} dBFS, attack {} ms, release {} ms)",
            num(&t["target_dbfs"]),
            num(&t["max_gain_db"]),
            num(&t["gate_dbfs"]),
            num(&t["attack_ms"]),
            num(&t["release_ms"])
        )
    } else {
        "AGC off".to_string()
    };
    format!("hi-res on · {activity} · {agc}")
}

fn settings_line(settings: &Value) -> String {
    let Some(map) = settings.as_object() else {
        return "none reported".to_string();
    };
    map.iter()
        .filter(|(k, _)| !k.starts_with("eq"))
        .map(|(k, v)| format!("{k} {}", v.as_str().unwrap_or("?")))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Format an `info` reply. `codec` looks up the active codec for an address.
pub fn format_info(reply: &Value, codec: impl Fn(&str) -> Option<String>) -> String {
    let devices = reply["devices"].as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        return "No AirPods connected.\n".to_string();
    }
    let mut out = String::new();
    for d in &devices {
        let mac = d["mac"].as_str().unwrap_or("?");
        let id = &d["identity"];
        let name = text(&id["name"]).unwrap_or("AirPods");
        let model = match (text(&id["model"]), text(&id["model_number"])) {
            (Some(m), Some(n)) => format!("{m}, {n}"),
            (None, Some(n)) => n.to_string(),
            _ => "model unknown".to_string(),
        };
        out.push_str(&format!("{name}  ({model})\n"));
        row(&mut out, "Address", mac);
        if let Some(fw) = id["firmware"].as_array() {
            let mut versions: Vec<&str> = fw.iter().filter_map(text).collect();
            versions.dedup();
            if !versions.is_empty() {
                row(&mut out, "Firmware", versions.join(" / "));
            }
        }
        if let Some(hw) = text(&id["hardware_revision"]) {
            row(&mut out, "Hardware", hw);
        }
        let serials: Vec<String> = [
            ("case", "serial_number"),
            ("left", "left_serial_number"),
            ("right", "right_serial_number"),
        ]
        .iter()
        .filter_map(|(label, key)| text(&id[*key]).map(|s| format!("{label} {s}")))
        .collect();
        if !serials.is_empty() {
            row(&mut out, "Serials", serials.join(" · "));
        }
        row(&mut out, "Battery", battery_line(&d["battery"]));
        if let Some(ears) = d["ears"].as_array().filter(|e| !e.is_empty()) {
            let ears: Vec<&str> = ears.iter().filter_map(Value::as_str).collect();
            row(&mut out, "Ears", ears.join(" / "));
        }
        row(&mut out, "Audio", audio_line(&d["audio"], codec(mac).as_deref()));
        row(&mut out, "Microphone", mic_line(&d["hires_mic"]));
        let s = &d["settings"];
        if s["eq"].is_string() {
            let band = |k: &str| s[k].as_str().unwrap_or("?").to_string();
            row(
                &mut out,
                "EQ",
                format!("{} · low {} · mid {} · high {}", band("eq"), band("eq-low"), band("eq-mid"), band("eq-high")),
            );
        }
        row(&mut out, "Settings", settings_line(s));
        let head = match &d["head"] {
            Value::Null => "off".to_string(),
            h if h["calibrating"] == true => "on, calibrating".to_string(),
            h => format!("on · yaw {} · pitch {} · roll {}", h["yaw"], h["pitch"], h["roll"]),
        };
        row(&mut out, "Head tracking", head);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_and_unknown_model_numbers() {
        assert_eq!(model_name("A3053"), Some("AirPods 4"));
        assert_eq!(model_name("A3048"), Some("AirPods Pro 2 (USB-C)"));
        assert_eq!(model_name("A9999"), None);
    }

    #[test]
    fn report_lists_identity_battery_audio_and_settings() {
        let reply = json!({ "ok": true, "devices": [{
            "mac": "AA:BB:CC:DD:EE:FF",
            "identity": { "name": "Test Pods", "model": "AirPods 4", "model_number": "A3053",
                          "firmware": ["1.2.3", ""], "hardware_revision": "1.0.0",
                          "serial_number": "CASE1", "left_serial_number": "", "right_serial_number": "R1" },
            "battery": [
                { "component": "left", "level": 77, "status": "NotCharging" },
                { "component": "right", "level": 90, "status": "Charging" },
                { "component": "case", "level": 0, "status": "Disconnected" }
            ],
            "ears": ["InEar", "InEar"],
            "audio": { "owner": "this-device", "source": "11:22:33:44:55:66", "connected": ["11:22:33:44:55:66"] },
            "hires_mic": { "enabled": true, "active": false, "recorder": null, "level": 0.0, "agc": false },
            "settings": { "press-speed": "default", "eq": "on", "eq-low": "75", "eq-mid": "35", "eq-high": "75" }
        }]});
        let text = format_info(&reply, |_| Some("AAC".to_string()));
        assert!(text.starts_with("Test Pods  (AirPods 4, A3053)\n"));
        assert!(text.contains("L 77%  R 90% (charging)"));
        assert!(!text.contains("case 0%"), "disconnected components are hidden");
        assert!(text.contains("playing from this computer · A2DP AAC"));
        assert!(text.contains("hi-res on · idle · AGC off"));
        assert!(text.contains("on · low 75 · mid 35 · high 75"));
        assert!(text.contains("press-speed default"));
        assert!(text.contains("Serials        case CASE1 · right R1"));
        assert!(text.contains("Head tracking  off"));
    }

    #[test]
    fn mic_line_prints_whole_numbers_without_decimals() {
        let mic = json!({ "enabled": true, "active": true, "recorder": "vesktop", "agc": true,
            "agc_tuning": { "target_dbfs": -12.0, "max_gain_db": 18.0, "gate_dbfs": -50.0,
                            "attack_ms": 5.0, "release_ms": 400.0 } });
        assert_eq!(
            mic_line(&mic),
            "hi-res on · live (vesktop) · AGC on (target -12 dBFS, boost 18 dB, gate -50 dBFS, attack 5 ms, release 400 ms)"
        );
    }

    #[test]
    fn report_without_devices() {
        assert_eq!(
            format_info(&json!({ "ok": true, "devices": [] }), |_| None),
            "No AirPods connected.\n"
        );
    }
}
