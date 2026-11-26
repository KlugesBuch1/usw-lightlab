# usw-lightlab

Desktop app for simulating and deploying LED animations on a UniFi USW-Pro-Max-48 switch. Allows previewing shell scripts locally and running them on the switch over SSH.

## Prerequisites

- Windows 10/11 and [Rust](https://rustup.rs).
- WSL with a Linux distro for preview only (`wsl --install`). If WSL is missing, only Preview is disabled.
- The switch must allow root SSH.

## How to Run

```
cargo run --release
```

`animations/kitt.sh` is included and loads automatically on start.

## How to Use

1. Pick an animation script with **Browse**.
2. **Preview** runs it in WSL in emit mode and simulates the switch port lights locally.
3. Real switch: fill in host, port and user, choose an authentication method, then **Trust and connect**.
4. **Test** runs the animation and streams its output. **Keep running** leaves it running after you disconnect. **Kill** stops it and turns all LEDs off.

Connection settings and pinned host keys are saved in `%APPDATA%\lightlab`. Passwords are only stored in Windows Credential Manager.

## Writing an animation

On the switch, LED colors are controlled through two files:

- `/proc/led/led_mode` — write `0` to let the script take over the LEDs, `1` to hand them back to the firmware.
- `/proc/led/led_color` — one text line per change: `<port> <channel> <value>`.

```
7 g 65535
```

That's port 7, channel `g`, full brightness. Port is 1–48, channel is `r`, `g` or `b`, and value is 0–65535. `animations/kitt.sh` shows the pattern: write a line for every port, wait a few milliseconds, repeat.

The catch: `/proc/led` only exists on the switch, so the same script can't run on your PC. Preview therefore starts it with `LIGHTLAB_EMIT=1`. The script prints those lines instead of writing them, and the app draws the ports from that output. The `mode 0` / `mode 1` lines are ignored in preview.

A script written this way runs both locally and on the switch.
