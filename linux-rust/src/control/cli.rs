//! Command-line interface to the running LibrePods instance. Every command has
//! readable output by default and JSON with the global `--json` flag. Exit
//! codes: 0 success, 1 failure while talking to LibrePods or the AirPods,
//! 2 invalid input (nothing is changed).

use std::io::Read;
use std::time::Duration;

use clap::Subcommand;
use serde_json::{Map, Value, json};

use crate::control::report::{bluetooth_codec, format_info};
use crate::control::server::request;
use crate::control::settings::{SettingSpec, all_settings, validate};

const EXIT_FAILURE: i32 = 1;
const EXIT_USAGE: i32 = 2;
const HEAD_WATCH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Subcommand)]
pub enum Command {
    /// Everything known about the connected AirPods
    Info,
    /// Show one setting's value, or every setting
    #[command(
        after_help = "Examples:\n  librepods get\n  librepods get press-speed\n  \
        librepods --json get\n\nRun `librepods settings` for all setting names."
    )]
    Get {
        /// Setting name, e.g. press-speed (omit to list every setting)
        setting: Option<String>,
    },
    /// Change one or more settings: NAME VALUE [NAME VALUE ...]
    #[command(after_help = "Examples:\n  librepods set press-speed slower\n  \
        librepods set eq-mid 50 agc-target -14\n  librepods set hires-mic off\n\n\
        All values are checked before anything changes. On/off settings also accept\n\
        true/false, yes/no, 1/0 and enable/disable. Run `librepods settings` for\n\
        every setting and its accepted values.")]
    Set {
        /// Alternating setting names and values
        #[arg(value_name = "NAME VALUE", num_args = 0.., allow_hyphen_values = true)]
        pairs: Vec<String>,
    },
    /// List every setting with its accepted values
    Settings,
    /// Show or change the custom EQ
    #[command(after_help = "Examples:\n  librepods eq             show the EQ\n  \
        librepods eq off\n  librepods eq 75 35 75    low mid high, each 0..100, 50 = flat")]
    Eq {
        /// on, off, or three band values LOW MID HIGH
        #[arg(num_args = 0..=3)]
        args: Vec<String>,
    },
    /// Head tracking: show the pose, switch it on or off, or stream poses
    #[command(after_help = "Examples:\n  librepods head on\n  \
        librepods head watch          yaw/pitch/roll until Ctrl+C\n  \
        librepods --json head watch   one JSON object per line\n\n\
        Angles are degrees from the position at start: yaw = turn, pitch = nod,\n\
        roll = tilt; turning left, looking up and tilting left are positive.")]
    Head {
        #[arg(value_parser = ["on", "off", "watch"])]
        action: Option<String>,
    },
    /// Rename the AirPods (shows the current name when given none)
    Rename {
        /// New name, up to 32 bytes
        #[arg(num_args = 0..)]
        name: Vec<String>,
    },
    /// Print every writable setting as JSON, ready for `apply`
    Export,
    /// Apply settings from a JSON object, such as one written by `export`
    #[command(
        after_help = "Examples:\n  librepods export > pods.json\n  librepods apply pods.json\n  \
        echo '{\"press-speed\": \"slower\", \"agc\": true}' | librepods apply -"
    )]
    Apply {
        /// JSON file, or - for standard input
        file: Option<String>,
    },
    /// Recent AirPods packets LibrePods does not recognise
    Packets {
        /// `clear`, or an opcode such as 0x55 to filter
        arg: Option<String>,
    },
    /// Print a shell completion script
    #[command(after_help = "Install for zsh:\n  \
        librepods completions zsh > ~/.local/share/zsh/site-functions/_librepods\n  \
        (that directory must be in $fpath; then start a new shell)")]
    Completions {
        #[arg(value_parser = ["zsh"])]
        shell: Option<String>,
    },
    /// Send a raw request to the control socket
    Ctl {
        #[arg(num_args = 0.., allow_hyphen_values = true)]
        request: Vec<String>,
    },
    /// Candidates for shell completion
    #[command(name = "__complete", hide = true)]
    Complete {
        #[arg(num_args = 0..)]
        args: Vec<String>,
    },
}

/// Run a command; returns the process exit code.
pub fn run(command: &Command, json: bool) -> i32 {
    let result = match command {
        Command::Info => info(json),
        Command::Get { setting } => get(setting.as_deref(), json),
        Command::Set { pairs } => set(pairs, json),
        Command::Settings => {
            settings_list(json);
            Ok(())
        }
        Command::Eq { args } => eq(args, json),
        Command::Head { action } => head(action.as_deref(), json),
        Command::Rename { name } => rename(name, json),
        Command::Export => export(),
        Command::Apply { file } => apply(file.as_deref(), json),
        Command::Packets { arg } => packets(arg.as_deref(), json),
        Command::Completions { shell } => completions(shell.as_deref()),
        Command::Ctl { request } => ctl(request),
        Command::Complete { args } => {
            complete(args);
            Ok(())
        }
    };
    match result {
        Ok(()) => 0,
        Err(Failure::Usage(msg)) => {
            eprintln!("{msg}");
            EXIT_USAGE
        }
        Err(Failure::Runtime(msg)) => {
            eprintln!("error: {msg}");
            EXIT_FAILURE
        }
    }
}

enum Failure {
    /// Invalid input; nothing was changed.
    Usage(String),
    /// LibrePods or the AirPods could not complete the request.
    Runtime(String),
}

type CliResult = Result<(), Failure>;

/// One control-socket request; a reply with `ok: false` becomes an error.
fn call(line: &str) -> Result<Value, Failure> {
    let reply = request(line).map_err(Failure::Runtime)?;
    let value: Value = serde_json::from_str(&reply)
        .map_err(|e| Failure::Runtime(format!("unreadable reply from LibrePods: {e}")))?;
    if value["ok"] == true {
        Ok(value)
    } else {
        let msg = value["error"].as_str().unwrap_or("request failed");
        Err(Failure::Runtime(msg.to_string()))
    }
}

fn first_device(status: &Value) -> Result<&Value, Failure> {
    status["devices"]
        .as_array()
        .and_then(|d| d.first())
        .ok_or_else(|| Failure::Runtime("no AirPods connected".to_string()))
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn settings_table(specs: &[SettingSpec]) -> String {
    let width = specs.iter().map(|s| s.name.len()).max().unwrap_or(0);
    let accepted_width = specs.iter().map(|s| s.accepted.len()).max().unwrap_or(0);
    specs
        .iter()
        .map(|s| {
            let note = if s.writable { "" } else { " (read-only)" };
            format!(
                "  {:<width$}  {:<accepted_width$}  {}{note}\n",
                s.name, s.accepted, s.about
            )
        })
        .collect()
}

fn settings_list(json: bool) {
    let specs = all_settings();
    if json {
        let map: Map<String, Value> = specs
            .iter()
            .map(|s| {
                let entry =
                    json!({ "accepted": s.accepted, "about": s.about, "writable": s.writable });
                (s.name.to_string(), entry)
            })
            .collect();
        print_json(&Value::Object(map));
    } else {
        print!(
            "Settings (librepods get NAME / librepods set NAME VALUE):\n\n{}",
            settings_table(&specs)
        );
    }
}

fn info(json: bool) -> CliResult {
    let reply = call("info")?;
    if json {
        print_json(&reply);
    } else {
        print!("{}", format_info(&reply, bluetooth_codec));
    }
    Ok(())
}

fn get(setting: Option<&str>, json: bool) -> CliResult {
    let specs = all_settings();
    if let Some(name) = setting
        && !specs.iter().any(|s| s.name == name)
    {
        return Err(Failure::Usage(format!(
            "unknown setting '{name}'. Settings:\n\n{}",
            settings_table(&specs)
        )));
    }
    let status = call("status")?;
    let current = &first_device(&status)?["settings"];
    match setting {
        Some(name) => {
            let value = current.get(name).cloned().unwrap_or(Value::Null);
            if json {
                print_json(&json!({ name: value }));
            } else {
                println!(
                    "{}",
                    value.as_str().unwrap_or("not reported by these AirPods")
                );
            }
        }
        None if json => print_json(current),
        None => {
            let width = specs.iter().map(|s| s.name.len()).max().unwrap_or(0);
            for spec in &specs {
                let value = current[spec.name].as_str().unwrap_or("-");
                println!("{:<width$}  {value:<10}  {}", spec.name, spec.about);
            }
        }
    }
    Ok(())
}

/// Validate every pair, then apply them in order.
fn set_pairs(pairs: &[(String, String)], json: bool) -> CliResult {
    let mut checked = Vec::new();
    let mut problems = Vec::new();
    for (name, value) in pairs {
        match validate(name, value) {
            Ok(canonical) => checked.push((name.clone(), canonical)),
            Err(e) => problems.push(e),
        }
    }
    if !problems.is_empty() {
        return Err(Failure::Usage(format!(
            "{}\nNothing was changed. Run `librepods settings` for accepted values.",
            problems.join("\n")
        )));
    }
    let mut applied = Map::new();
    let mut failed = Map::new();
    for (name, value) in &checked {
        match call(&format!("set {name} {value}")) {
            Ok(_) => {
                applied.insert(name.clone(), json!(value));
            }
            Err(Failure::Runtime(e) | Failure::Usage(e)) => {
                failed.insert(name.clone(), json!(e));
            }
        }
    }
    if json {
        print_json(&json!({ "ok": failed.is_empty(), "applied": applied, "failed": failed }));
    } else {
        for (name, value) in &applied {
            println!("{name} = {}", value.as_str().unwrap_or_default());
        }
        for (name, err) in &failed {
            eprintln!("{name}: {}", err.as_str().unwrap_or_default());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Failure::Runtime(format!(
            "{} setting(s) could not be applied",
            failed.len()
        )))
    }
}

fn set(args: &[String], json: bool) -> CliResult {
    let specs = all_settings();
    if args.is_empty() {
        return Err(Failure::Usage(format!(
            "Usage: librepods set NAME VALUE [NAME VALUE ...]\n\nSettings:\n\n{}\n\
             Example: librepods set press-speed slower eq-mid 50",
            settings_table(&specs)
        )));
    }
    if args.len() % 2 == 1 {
        let last = &args[args.len() - 1];
        let hint = specs.iter().find(|s| s.name == last.as_str()).map_or_else(
            || format!("'{last}' is not a setting; run `librepods settings`."),
            |s| format!("{last} accepts: {}  ({})", s.accepted, s.about),
        );
        return Err(Failure::Usage(format!(
            "Missing a value for '{last}'.\n{hint}"
        )));
    }
    let pairs: Vec<(String, String)> = args
        .chunks(2)
        .map(|p| (p[0].clone(), p[1].clone()))
        .collect();
    set_pairs(&pairs, json)
}

fn eq(args: &[String], json: bool) -> CliResult {
    match args.len() {
        0 => {
            let status = call("status")?;
            let s = &first_device(&status)?["settings"];
            if s["eq"].is_null() {
                return Err(Failure::Runtime(
                    "these AirPods have not reported a custom EQ".to_string(),
                ));
            }
            if json {
                print_json(&json!({
                    "eq": s["eq"], "eq-low": s["eq-low"], "eq-mid": s["eq-mid"], "eq-high": s["eq-high"]
                }));
            } else {
                let v = |k: &str| s[k].as_str().unwrap_or("?").to_string();
                println!(
                    "{} · low {} · mid {} · high {}",
                    v("eq"),
                    v("eq-low"),
                    v("eq-mid"),
                    v("eq-high")
                );
                println!(
                    "Change: librepods eq on|off   or   librepods eq LOW MID HIGH (0..100, 50 = flat)"
                );
            }
            Ok(())
        }
        1 => set_pairs(&[("eq".to_string(), args[0].clone())], json),
        3 => {
            let pairs: Vec<(String, String)> = ["eq-low", "eq-mid", "eq-high"]
                .iter()
                .zip(args)
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect();
            set_pairs(&pairs, json)
        }
        _ => Err(Failure::Usage(
            "Usage: librepods eq [on|off | LOW MID HIGH]  (bands 0..100, 50 = flat)".to_string(),
        )),
    }
}

fn head_pose() -> Result<Value, Failure> {
    let status = call("status")?;
    Ok(first_device(&status)?["head"].clone())
}

fn describe_pose(pose: &Value) -> String {
    match pose {
        Value::Null => "off".to_string(),
        p if p["calibrating"] == true => "calibrating (hold still)".to_string(),
        p if p["yaw"].is_null() => "on, waiting for data".to_string(),
        p => format!(
            "yaw {:+6.1}°  pitch {:+6.1}°  roll {:+6.1}°",
            p["yaw"].as_f64().unwrap_or(0.0),
            p["pitch"].as_f64().unwrap_or(0.0),
            p["roll"].as_f64().unwrap_or(0.0)
        ),
    }
}

fn head(action: Option<&str>, json: bool) -> CliResult {
    match action {
        None => {
            let pose = head_pose()?;
            if json {
                print_json(&pose);
            } else {
                println!("{}", describe_pose(&pose));
                println!("Change: librepods head on|off   Stream: librepods head watch");
            }
            Ok(())
        }
        Some(state @ ("on" | "off")) => {
            call(&format!("head-tracking {state}"))?;
            if json {
                print_json(&json!({ "head-tracking": state }));
            } else {
                println!("head tracking {state}");
            }
            Ok(())
        }
        Some(_) => head_watch(json),
    }
}

/// Stream poses until Ctrl+C; tracking started here is stopped again on exit.
fn head_watch(json: bool) -> CliResult {
    let started_here = head_pose()?.is_null();
    if started_here {
        call("head-tracking on")?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Failure::Runtime(e.to_string()))?;
    let outcome: CliResult = runtime.block_on(async {
        let mut ticker = tokio::time::interval(HEAD_WATCH_INTERVAL);
        let interrupt = tokio::signal::ctrl_c();
        tokio::pin!(interrupt);
        loop {
            tokio::select! {
                _ = &mut interrupt => return Ok::<(), Failure>(()),
                _ = ticker.tick() => {
                    let pose = tokio::task::spawn_blocking(head_pose)
                        .await
                        .map_err(|e| Failure::Runtime(e.to_string()))??;
                    if json {
                        println!("{pose}");
                    } else {
                        println!("{}", describe_pose(&pose));
                    }
                }
            }
        }
    });
    if started_here {
        let _ = call("head-tracking off");
    }
    outcome
}

fn rename(name: &[String], json: bool) -> CliResult {
    if name.is_empty() {
        let reply = call("info")?;
        let current = first_device(&reply)?["identity"]["name"].clone();
        if json {
            print_json(&json!({ "name": current }));
        } else {
            println!("{}", current.as_str().unwrap_or("unknown"));
            println!("Change: librepods rename NEW NAME (up to 32 bytes)");
        }
        return Ok(());
    }
    let new_name = name.join(" ");
    let reply = call(&format!("rename {new_name}"))?;
    if json {
        print_json(&reply);
    } else {
        println!("renamed to {}", reply["name"].as_str().unwrap_or(&new_name));
    }
    Ok(())
}

fn export() -> CliResult {
    let status = call("status")?;
    let current = &first_device(&status)?["settings"];
    let exported: Map<String, Value> = all_settings()
        .into_iter()
        .filter(|s| s.writable)
        .filter_map(|s| current.get(s.name).map(|v| (s.name.to_string(), v.clone())))
        .collect();
    print_json(&Value::Object(exported));
    Ok(())
}

/// Setting value from JSON as text: strings as-is, numbers as written,
/// booleans as on/off.
fn json_value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "on" } else { "off" }.to_string()),
        _ => None,
    }
}

/// Setting pairs from a JSON object of setting names to values.
fn pairs_from_json(text: &str, source: &str) -> Result<Vec<(String, String)>, Failure> {
    let object: Map<String, Value> = serde_json::from_str(text)
        .map_err(|e| Failure::Usage(format!("{source} is not a JSON object of settings: {e}")))?;
    object
        .iter()
        .map(|(name, value)| {
            json_value_text(value)
                .map(|text| (name.clone(), text))
                .ok_or_else(|| {
                    Failure::Usage(format!("{name}: value must be a string, number or boolean"))
                })
        })
        .collect()
}

fn apply(file: Option<&str>, json: bool) -> CliResult {
    let Some(file) = file else {
        return Err(Failure::Usage(
            "Usage: librepods apply FILE   (or - to read standard input)\n\n\
             The file is a JSON object of setting names to values, e.g.\n  \
             {\"press-speed\": \"slower\", \"eq-mid\": 50, \"agc\": true}\n\n\
             `librepods export` writes the current settings in this format."
                .to_string(),
        ));
    };
    let text = if file == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map(|_| text)
    } else {
        std::fs::read_to_string(file)
    }
    .map_err(|e| Failure::Usage(format!("cannot read {file}: {e}")))?;
    let pairs = pairs_from_json(&text, file)?;
    set_pairs(&pairs, json)
}

fn packets(arg: Option<&str>, json: bool) -> CliResult {
    let line = match arg {
        None => "packets".to_string(),
        Some(a) => format!("packets {a}"),
    };
    let reply = call(&line)?;
    if json {
        print_json(&reply);
        return Ok(());
    }
    if reply["cleared"] == true {
        println!("cleared");
        return Ok(());
    }
    let list = reply["packets"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("no unrecognised packets recorded");
    }
    for p in list {
        println!(
            "{}  {}  {}",
            p["t"],
            p["opcode"].as_str().unwrap_or("?"),
            p["hex"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

fn ctl(request_words: &[String]) -> CliResult {
    if request_words.is_empty() {
        return Err(Failure::Usage(
            "Usage: librepods ctl REQUEST\n\nRequests: status | info | settings | set NAME VALUE | \
             head-tracking on|off | rename NAME | packets [clear|OPCODE]"
                .to_string(),
        ));
    }
    let reply = request(&request_words.join(" ")).map_err(Failure::Runtime)?;
    println!("{reply}");
    let ok = serde_json::from_str::<Value>(&reply).is_ok_and(|v| v["ok"] == true);
    if ok {
        Ok(())
    } else {
        Err(Failure::Runtime("request failed".to_string()))
    }
}

fn completions(shell: Option<&str>) -> CliResult {
    match shell {
        Some("zsh") => {
            print!("{ZSH_COMPLETION}");
            Ok(())
        }
        _ => Err(Failure::Usage(
            "Usage: librepods completions zsh\n\nInstall:\n  \
             librepods completions zsh > ~/.local/share/zsh/site-functions/_librepods"
                .to_string(),
        )),
    }
}

/// Completion candidates. `settings` prints NAME:DESCRIPTION lines; `values
/// NAME` prints the choices for a setting (none for numeric ranges);
/// `accepted NAME` prints the accepted-values text.
fn complete(args: &[String]) {
    let specs = all_settings();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["settings"] => {
            for s in specs.iter().filter(|s| s.writable) {
                println!("{}:{} [{}]", s.name, s.about.replace(':', " "), s.accepted);
            }
        }
        ["values", name] => {
            if let Some(s) = specs.iter().find(|s| s.name == *name)
                && !s.accepted.contains("..")
            {
                for choice in s.accepted.split('|') {
                    println!("{choice}");
                }
            }
        }
        ["accepted", name] => {
            if let Some(s) = specs.iter().find(|s| s.name == *name) {
                println!("{}", s.accepted);
            }
        }
        _ => {}
    }
}

const ZSH_COMPLETION: &str = r#"#compdef librepods
# zsh completion for librepods. Install with:
#   librepods completions zsh > ~/.local/share/zsh/site-functions/_librepods

_librepods_settings() {
  local -a settings
  settings=(${(f)"$(librepods __complete settings 2>/dev/null)"})
  _describe -t settings 'setting' settings
}

_librepods_values() {
  local -a values
  values=(${(f)"$(librepods __complete values $1 2>/dev/null)"})
  if (( ${#values} )); then
    _describe -t values "value for $1" values
  else
    _message "value for $1: $(librepods __complete accepted $1 2>/dev/null)"
  fi
}

_librepods() {
  local -a commands
  commands=(
    'info:everything known about the connected AirPods'
    'get:show one setting or every setting'
    'set:change settings (NAME VALUE pairs)'
    'settings:list settings and accepted values'
    'eq:show or change the custom EQ'
    'head:head tracking (on, off, watch)'
    'rename:rename the AirPods'
    'export:print settings as JSON'
    'apply:apply settings from a JSON file'
    'packets:recent unrecognised packets'
    'completions:print a shell completion script'
    'ctl:raw control-socket request'
  )
  local i=2
  while [[ ${words[i]} == -* ]]; do (( i++ )); done
  if (( CURRENT <= i )); then
    if [[ ${words[CURRENT]} == -* ]]; then
      _values 'option' '--json[machine-readable JSON output]' '--help[show help]'
    else
      _describe -t commands 'librepods command' commands
    fi
    return
  fi
  local pos=$(( CURRENT - i ))
  case ${words[i]} in
    get) (( pos == 1 )) && _librepods_settings ;;
    set)
      if (( pos % 2 == 1 )); then _librepods_settings
      else _librepods_values ${words[CURRENT-1]}; fi ;;
    eq) (( pos == 1 )) && _values 'eq' on off ;;
    head) (( pos == 1 )) && _values 'action' on off watch ;;
    apply) _files ;;
    packets) (( pos == 1 )) && _values 'packets' clear ;;
    completions) (( pos == 1 )) && _values 'shell' zsh ;;
  esac
}

_librepods "$@"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_values_become_setting_text() {
        assert_eq!(json_value_text(&json!(true)).as_deref(), Some("on"));
        assert_eq!(json_value_text(&json!(false)).as_deref(), Some("off"));
        assert_eq!(json_value_text(&json!(-14)).as_deref(), Some("-14"));
        assert_eq!(json_value_text(&json!("slower")).as_deref(), Some("slower"));
        assert_eq!(json_value_text(&json!([1])), None);
    }

    #[test]
    fn apply_file_parses_names_and_values() {
        let pairs = pairs_from_json(r#"{"agc": true, "eq-mid": 50}"#, "test")
            .ok()
            .unwrap();
        assert!(pairs.contains(&("agc".to_string(), "on".to_string())));
        assert!(pairs.contains(&("eq-mid".to_string(), "50".to_string())));
        assert!(matches!(
            pairs_from_json("[1]", "test"),
            Err(Failure::Usage(_))
        ));
        assert!(matches!(
            pairs_from_json(r#"{"agc": null}"#, "test"),
            Err(Failure::Usage(_))
        ));
    }

    #[test]
    fn invalid_set_changes_nothing_and_lists_options() {
        // Validation fails before any request is sent to LibrePods.
        let args = ["press-speed".to_string(), "warp".to_string()];
        match set(&args, false) {
            Err(Failure::Usage(msg)) => assert!(msg.contains("Nothing was changed"), "{msg}"),
            _ => panic!("expected a usage error"),
        }
    }

    #[test]
    fn missing_value_names_accepted_values() {
        match set(&["agc-target".to_string()], false) {
            Err(Failure::Usage(msg)) => assert!(msg.contains("-30..-3"), "{msg}"),
            _ => panic!("expected a usage error"),
        }
    }

    #[test]
    fn empty_set_lists_settings() {
        match set(&[], false) {
            Err(Failure::Usage(msg)) => {
                assert!(msg.contains("press-speed"));
                assert!(msg.contains("agc-release"));
            }
            _ => panic!("expected a usage error"),
        }
    }
}
