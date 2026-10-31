# batteries_app

Battery levels of your keyboards, mice, headsets and controllers, shown as a small strip on the
Windows 11 taskbar of the monitor you choose.

```
cargo run --release --bin batteries-bar      # taskbar strip + tray icon
cargo run --release --bin batteries          # one-off table in the terminal (--json for JSON)
```

Build needs the MSVC Rust toolchain (`rustup default stable-x86_64-pc-windows-msvc`), Rust 1.84+.

## The strip
- One icon per device (keyboard, mouse, headset, headphones, earbuds, gamepad) with its percentage;
  ⚡ while charging, red below the low-battery level. Disconnected devices are hidden (optionally
  shown dimmed). When two shown devices share an icon, a short name tag is added.
- Sits inside the taskbar on the right (next to the tray/clock) or the left, with an optional
  offset; hides with the taskbar (full-screen apps, auto-hide). Follows the light/dark theme.
- Hover for full names, connection and source. Double-click (or the tray icon) for Settings;
  right-click for the menu.
- Tray menu: Settings, Refresh now, Show on (monitor), Position, Devices, Start with Windows
  (off by default), Quit.
- Settings (`batteries-bar --settings`, an egui window): live preview; per device show/hide,
  display mode, icon, short tag, order; monitor, position, text size, compact two-row mode,
  low-battery level, poll interval. Hovering a device outlines it on the strip.
- Batteries are polled in the background every 60 s (configurable).
- Config: `%APPDATA%\batteries_app\config.toml`.

## Supported devices
| Source | Devices |
|---|---|
| Windows Bluetooth battery property | Bluetooth classic headsets/earbuds (e.g. SOUNDPEATS H3) |
| BLE Battery Service (GATT) | BLE keyboards/mice, every battery level characteristic (split keyboards) |
| Logitech HID++ 2.0 | Devices on Unifying/Bolt/Lightspeed receivers (MX Master 3, K580, ...) |
| SteelSeries | Arctis Nova 7 family dongles (protocol from HeadsetControl) |
| Sony | DualShock 4 over Bluetooth/USB |
| Keychron Link | Lists paired devices; battery only where the firmware reports it |

Device type is detected in layers (BLE appearance, Bluetooth class of device, Windows device
category, then a model-name table); Settings → Details shows which one decided.
Every provider only sends read/query requests to devices.

Per-device findings, protocols and known limits (Glove80 right half, Keychron K3 HE over 2.4 GHz,
MX Master 3 while charging): [device-inventory.md](device-inventory.md).

## Layout
- `crates/battery-core`: `Battery`, `Level`, `Kind`, the `Provider` trait, kind detection, merging.
- `crates/providers`: one module per source (`pnp`, `gatt`, `logitech`, `steelseries`,
  `dualshock`, `keychron`).
- `crates/cli`: the `batteries` binary.
- `crates/bar`: the `batteries-bar` taskbar strip, tray and settings window.
