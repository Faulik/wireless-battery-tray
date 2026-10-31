# batteries_app

Shows the battery of your keyboard, mouse and headphones on Windows. Milestone 1 is a CLI; the taskbar UI comes next.

```
cargo run --bin batteries            # table
cargo run --bin batteries -- --json  # machine-readable
```

Set `BATTERIES_DEBUG=1` to see per-provider timing and GATT details on stderr.

## Layout
- `crates/battery-core`: `Battery`, `Level`, `Kind`, the `Provider` trait, and merging when two providers see the same device (higher `priority()` wins per `device_id`).
- `crates/providers`: one module per source:
  - `pnp`: Windows' own Bluetooth battery property (`DEVPKEY_Bluetooth_Battery`), whole devnode tree.
  - `gatt`: BLE Battery Service via WinRT; every Battery Level characteristic (split keyboards).
  - `logitech`: HID++ 2.0 through Unifying/Bolt receivers (features 0x1004 / 0x1000 / 0x1001).
  - `steelseries`: Arctis Nova 7 family dongles (protocol from HeadsetControl).
- `crates/cli`: the `batteries` binary.

Supported devices and per-device notes: [device-inventory.md](device-inventory.md).

Build needs the MSVC Rust toolchain (`rustup default stable-x86_64-pc-windows-msvc`).
