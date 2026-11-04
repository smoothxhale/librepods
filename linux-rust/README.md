# LibrePods for Linux (Rust)

A native Linux companion for AirPods: battery, ear detection, device settings,
custom EQ, head tracking and the high-quality AirPods microphone.

## Building

Requirements: Rust (2024 edition), clang (for bindgen), and development files
for BlueZ/D-Bus, PulseAudio (`libpulse`) and FFmpeg's `libavcodec`.

```bash
cargo build --release
./target/release/librepods
```

If bindgen cannot find libclang, point `LIBCLANG_PATH` at the directory that
contains `libclang.so`.

Run without a command, `librepods` starts the application (`--no-tray` and
`--start-minimized` are available for tiling window managers).

## Command line

While the application is running, the same binary reads and changes its state
through a private control socket (`$XDG_RUNTIME_DIR/librepods/control.sock`,
owner-only permissions).

```bash
librepods info                        # identity, firmware, battery, audio, mic, EQ, settings
librepods settings                    # every setting with accepted values
librepods get                         # every current value
librepods get press-speed             # one value, printed bare for scripts
librepods set press-speed slower eq-mid 50
librepods eq 75 35 75                 # low mid high, 0..100, 50 = flat
librepods eq off
librepods head watch                  # yaw / pitch / roll until Ctrl+C
librepods rename "My AirPods"
librepods export > airpods.json       # every writable setting as JSON
librepods apply airpods.json          # or: echo '{"agc": true}' | librepods apply -
```

- `--json` on any command prints machine-readable output.
- Every value is validated before anything is sent; if one pair is invalid,
  nothing changes.
- On/off settings accept `on/off`, `true/false`, `yes/no`, `1/0` and
  `enable/disable`.
- Running a command without its arguments prints the accepted settings and
  values, and `librepods COMMAND --help` shows examples.
- Exit status: `0` success, `1` LibrePods or the AirPods could not complete the
  request, `2` invalid input.

### Settings

| Setting | Values | Description |
|---|---|---|
| `mic-mode` | `auto\|right\|left` | which bud's microphone is used |
| `ear-detection` | `on\|off` | react to taking buds out |
| `press-speed` | `default\|slower\|slowest` | how fast stem presses must be |
| `press-hold` | `default\|slower\|slowest` | how long a press-and-hold lasts |
| `chime-volume` | `0..100` | volume of connect and low-battery chimes |
| `in-case-tone` | `on\|off` | tone when the buds go into the case |
| `sleep-detection` | `on\|off` | pause media when you fall asleep |
| `adaptive-volume` | `on\|off` | adapt volume to your surroundings |
| `eq`, `eq-low`, `eq-mid`, `eq-high` | `on\|off`, `0..100` | custom EQ on models that support it |
| `hires-mic` | `on\|off` | high-quality microphone, exposed as the `AirPodsHiRes` input |
| `agc` | `on\|off` | automatic gain for the high-quality microphone |
| `agc-target` | `-30..-3` | speech level the gain aims for, dBFS |
| `agc-max-boost` | `0..30` | largest boost applied, dB |
| `agc-gate` | `-80..-20` | input below this level counts as background, dBFS |
| `agc-attack` | `1..100` | how fast gain drops when you get loud, ms |
| `agc-release` | `50..3000` | how fast gain recovers after you get quiet, ms |

`call-management` and `uplink-eq` are reported read-only. A setting the
connected model does not report shows as `-`.

### Shell completion (zsh)

```bash
mkdir -p ~/.local/share/zsh/site-functions
librepods completions zsh > ~/.local/share/zsh/site-functions/_librepods
```

Add that directory to `fpath` before `compinit` in `~/.zshrc` and start a new
shell. Completion covers commands, setting names with descriptions, and each
setting's values.

## Tested hardware

Settings, custom EQ, head tracking and the high-quality microphone have been
tested on AirPods 4 (non-ANC). Other models may report a different subset of
settings; reports from other models are welcome.

## Protocol notes

Observations about AACP packets that LibrePods does not act on are collected in
[`docs/aacp_observations.md`](../docs/aacp_observations.md).
