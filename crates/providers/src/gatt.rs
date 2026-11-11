//! BLE Battery Service (0x180F) over WinRT GATT.
//!
//! Every Battery Level characteristic (0x2A19) in every Battery Service instance is read, so
//! ZMK split keyboards built with `CONFIG_ZMK_SPLIT_BLE_CENTRAL_BATTERY_LEVEL_PROXY` show one
//! entry per half. Without that option only the central half is visible.
//!
//! Service discovery uses the cache: uncached discovery reports Unreachable for HID keyboards
//! that the Windows HID driver owns. Values are read uncached while the device is connected;
//! when it is not, the value Windows cached from the last connection is returned and the entry
//! is marked disconnected.
//!
//! Every WinRT call is awaited with a deadline: right after resume from hibernate the Bluetooth
//! stack can leave one pending forever, which used to freeze the whole poll.

use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use battery_core::{Battery, Kind, KindSource, Level, Provider};
use windows::Devices::Bluetooth::GenericAttributeProfile::{GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids, GattSession};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows::Foundation::{AsyncStatus, IAsyncOperation};
use windows::Storage::Streams::DataReader;

/// Longest one WinRT call may take. Uncached reads from a sleeping device take a few seconds.
const OP_TIMEOUT: Duration = Duration::from_secs(5);

/// Waits for `op` like `get()`, but gives up and cancels it after `OP_TIMEOUT`.
fn wait<T: windows::core::RuntimeType + 'static>(op: IAsyncOperation<T>) -> Result<T> {
    let deadline = Instant::now() + OP_TIMEOUT;
    while op.Status()? == AsyncStatus::Started {
        if Instant::now() >= deadline {
            let _ = op.Cancel();
            bail!("Bluetooth call got no answer within {OP_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(op.GetResults()?)
}

pub struct GattProvider;

impl Provider for GattProvider {
    fn name(&self) -> &'static str {
        "gatt"
    }
    fn priority(&self) -> u8 {
        2
    }
    fn poll(&self) -> Result<Vec<Battery>> {
        let selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
        let infos = wait(DeviceInformation::FindAllAsyncAqsFilter(&selector)?)?;
        let mut out = Vec::new();
        for info in infos {
            let Ok(dev) = wait(BluetoothLEDevice::FromIdAsync(&info.Id()?)?) else {
                continue;
            };
            out.extend(read_device(&dev)?);
        }
        Ok(out)
    }
}

fn read_device(dev: &BluetoothLEDevice) -> Result<Vec<Battery>> {
    let name = dev.Name()?.to_string();
    let connected = dev.ConnectionStatus()? == BluetoothConnectionStatus::Connected;
    debug!("gatt: {name} connected={connected}");

    // Holds the link open for the uncached reads below; dropped (released) when we return.
    let _session = if connected {
        let s = wait(GattSession::FromDeviceIdAsync(&dev.BluetoothDeviceId()?)?)?;
        s.SetMaintainConnection(true)?;
        Some(s)
    } else {
        None
    };

    let services = wait(dev.GetGattServicesForUuidWithCacheModeAsync(GattServiceUuids::Battery()?, BluetoothCacheMode::Cached)?)?;
    if services.Status()? != GattCommunicationStatus::Success {
        debug!("gatt: {name} battery service discovery: {:?}", services.Status()?);
        return Ok(Vec::new());
    }

    let modes: &[BluetoothCacheMode] = if connected { &[BluetoothCacheMode::Uncached, BluetoothCacheMode::Cached] } else { &[BluetoothCacheMode::Cached] };
    let mut levels = Vec::new();
    for service in services.Services()? {
        let chars = wait(service.GetCharacteristicsForUuidWithCacheModeAsync(GattCharacteristicUuids::BatteryLevel()?, BluetoothCacheMode::Cached)?)?;
        if chars.Status()? != GattCommunicationStatus::Success {
            continue;
        }
        for c in chars.Characteristics()? {
            let label = c.UserDescription().map(|s| s.to_string()).unwrap_or_default();
            for &mode in modes {
                let read = wait(c.ReadValueWithCacheModeAsync(mode)?)?;
                debug!("gatt: {name} handle {} {label:?} read {mode:?}: {:?}", c.AttributeHandle()?, read.Status()?);
                if read.Status()? == GattCommunicationStatus::Success {
                    let pct = DataReader::FromBuffer(&read.Value()?)?.ReadByte()?.min(100);
                    levels.push((label, pct));
                    break;
                }
            }
        }
    }

    let device_id = crate::bt_device_id(dev.BluetoothAddress()?);
    let appearance = dev.Appearance().and_then(|a| a.RawValue()).unwrap_or(0);
    let kind = appearance_kind(appearance);
    debug!("gatt: {name} appearance {appearance:#06x} -> {kind:?}");
    let kind_source = if kind == Kind::Other { KindSource::Provider } else { KindSource::BleAppearance };
    let multi = levels.len() > 1;
    Ok(levels
        .into_iter()
        .enumerate()
        .map(|(i, (label, pct))| Battery {
            device_id: device_id.clone(),
            name: name.clone(),
            part: match (label.is_empty(), multi) {
                (false, _) => Some(label),
                (true, true) if i == 0 => Some("Central".to_string()),
                (true, true) => Some(format!("#{}", i + 1)),
                (true, false) => None,
            },
            kind,
            kind_source,
            level: Level::Percent(pct),
            charging: None,
            connected: Some(connected),
            source: "ble-gatt",
        })
        .collect())
}

/// GAP appearance: category in bits 6..16, subcategory in bits 0..6 (Bluetooth Assigned Numbers).
fn appearance_kind(raw: u16) -> Kind {
    match (raw >> 6, raw & 0x3F) {
        (0x0F, 1) => Kind::Keyboard,
        (0x0F, 2) => Kind::Mouse,
        (0x0F, 3 | 4) => Kind::Gamepad,
        // 0x0940 Wearable Audio Device: 1 earbud, 2 headset, 3 headphones, 4 neckband.
        (0x25, 1) => Kind::Earbuds,
        (0x25, 3) => Kind::Headphones,
        (0x25, 4) => Kind::Neckband,
        (0x25, _) => Kind::Headset,
        _ => Kind::Other,
    }
}
