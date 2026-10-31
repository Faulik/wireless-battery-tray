//! Battery level Windows already knows: the `DEVPKEY_Bluetooth_Battery` devnode property.
//!
//! Classic Bluetooth headsets report it on a service child (e.g. the Hands-Free AG node),
//! not on the `BTHENUM\DEV_<addr>` node, so the whole devnode tree is walked and each hit
//! is mapped back to its device by Bluetooth address.

use std::collections::HashMap;

use anyhow::{bail, Result};
use battery_core::{Battery, Kind, Level, Provider};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Child, CM_Get_DevNode_PropertyW, CM_Get_Device_IDW, CM_Get_Sibling, CM_Locate_DevNodeW,
    CM_LOCATE_DEVNODE_NORMAL, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_FriendlyName, DEVPROPKEY, DEVPROPTYPE};

const DEVPKEY_BLUETOOTH_BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
    pid: 2,
};
const DEVPKEY_BLUETOOTH_CLASS_OF_DEVICE: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x2BD67D8B_8BEB_48D5_87E0_6CDA3428040A),
    pid: 10,
};

pub struct PnpProvider;

impl Provider for PnpProvider {
    fn name(&self) -> &'static str {
        "pnp"
    }
    fn priority(&self) -> u8 {
        1
    }
    fn poll(&self) -> Result<Vec<Battery>> {
        let nodes = all_devnodes()?;

        // Bluetooth address -> (friendly name, class of device) from the device's own node.
        let mut devices: HashMap<u64, (String, Option<u32>)> = HashMap::new();
        for &(inst, ref id) in &nodes {
            if id.starts_with("BTHENUM\\DEV_") || id.starts_with("BTHLE\\DEV_") {
                if let Some(addr) = bt_address(id) {
                    let name = prop_string(inst, &DEVPKEY_Device_FriendlyName).unwrap_or_else(|| id.clone());
                    let cod = prop_u32(inst, &DEVPKEY_BLUETOOTH_CLASS_OF_DEVICE);
                    devices.insert(addr, (name, cod));
                }
            }
        }

        let mut out = Vec::new();
        for &(inst, ref id) in &nodes {
            let Some(pct) = prop_bytes(inst, &DEVPKEY_BLUETOOTH_BATTERY).and_then(|b| b.first().copied()) else {
                continue;
            };
            let addr = bt_address(id);
            let (name, cod) = addr
                .and_then(|a| devices.get(&a).cloned())
                .unwrap_or_else(|| (prop_string(inst, &DEVPKEY_Device_FriendlyName).unwrap_or(id.clone()), None));
            out.push(Battery {
                device_id: addr.map(crate::bt_device_id).unwrap_or_else(|| format!("pnp:{id}")),
                name,
                part: None,
                kind: cod.map(kind_from_cod).unwrap_or(Kind::Other),
                level: Level::Percent(pct.min(100)),
                charging: None,
                connected: None,
                source: "windows-pnp",
            });
        }
        Ok(out)
    }
}

/// Bluetooth class of device: major class in bits 8..13, minor in bits 2..8.
fn kind_from_cod(cod: u32) -> Kind {
    match (cod >> 8) & 0x1F {
        0x04 => Kind::Headset,
        0x05 => match (cod >> 6) & 0x03 {
            1 => Kind::Keyboard,
            2 => Kind::Mouse,
            _ => Kind::Other,
        },
        _ => Kind::Other,
    }
}

/// Extract the 12-hex-digit address from Bluetooth instance ids such as
/// `BTHLE\DEV_0A1B2C3D4E5F\...` or `BTHENUM\{...}_LOCALMFG&005D\A&2B3C4D5E&0&112233445566_C00000000`.
fn bt_address(id: &str) -> Option<u64> {
    let hex12 = |s: &str| {
        (s.len() >= 12 && s.as_bytes()[..12].iter().all(u8::is_ascii_hexdigit))
            .then(|| u64::from_str_radix(&s[..12], 16).ok())
            .flatten()
    };
    if let Some(i) = id.find("DEV_") {
        return hex12(&id[i + 4..]);
    }
    if !id.starts_with("BTHENUM\\") && !id.starts_with("BTHLEDEVICE\\") {
        return None;
    }
    let last = id.rsplit('\\').next()?;
    let tail = last.rsplit('&').next()?;
    hex12(tail.split('_').next()?)
}

fn all_devnodes() -> Result<Vec<(u32, String)>> {
    let mut root = 0u32;
    // SAFETY: valid out-pointer; null device id means the root of the tree.
    let cr = unsafe { CM_Locate_DevNodeW(&mut root, PCWSTR::null(), CM_LOCATE_DEVNODE_NORMAL) };
    if cr != CR_SUCCESS {
        bail!("CM_Locate_DevNodeW failed: {}", cr.0);
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(inst) = stack.pop() {
        if let Some(id) = device_id(inst) {
            out.push((inst, id));
        }
        let mut child = 0u32;
        // SAFETY: valid out-pointer and devnode handle from the CM tree walk.
        if unsafe { CM_Get_Child(&mut child, inst, 0) } == CR_SUCCESS {
            stack.push(child);
            let mut sib = child;
            loop {
                let mut next = 0u32;
                // SAFETY: as above.
                if unsafe { CM_Get_Sibling(&mut next, sib, 0) } != CR_SUCCESS {
                    break;
                }
                stack.push(next);
                sib = next;
            }
        }
    }
    Ok(out)
}

fn device_id(inst: u32) -> Option<String> {
    let mut buf = [0u16; 512];
    // SAFETY: buffer is writable and its length is passed via the slice.
    if unsafe { CM_Get_Device_IDW(inst, &mut buf, 0) } != CR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

fn prop_bytes(inst: u32, key: &DEVPROPKEY) -> Option<Vec<u8>> {
    let mut ty = DEVPROPTYPE::default();
    let mut size = 0u32;
    // SAFETY: size query with no buffer, then a read into a buffer of exactly that size.
    unsafe {
        CM_Get_DevNode_PropertyW(inst, key, &mut ty, None, &mut size, 0);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if CM_Get_DevNode_PropertyW(inst, key, &mut ty, Some(buf.as_mut_ptr()), &mut size, 0) != CR_SUCCESS {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

fn prop_u32(inst: u32, key: &DEVPROPKEY) -> Option<u32> {
    let b = prop_bytes(inst, key)?;
    Some(u32::from_le_bytes(b.get(..4)?.try_into().ok()?))
}

fn prop_string(inst: u32, key: &DEVPROPKEY) -> Option<String> {
    let b = prop_bytes(inst, key)?;
    let wide: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let len = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    Some(String::from_utf16_lossy(&wide[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bluetooth_addresses() {
        assert_eq!(bt_address(r"BTHLE\DEV_0A1B2C3D4E5F\8&11223344&0&0A1B2C3D4E5F"), Some(0x0A1B2C3D4E5F));
        assert_eq!(
            bt_address(r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_LOCALMFG&005D\A&2B3C4D5E&0&112233445566_C00000000"),
            Some(0x112233445566)
        );
        assert_eq!(bt_address(r"USB\VID_046D&PID_C52B\8&1234ABC&0&5"), None);
    }

    #[test]
    fn class_of_device_kinds() {
        assert_eq!(kind_from_cod(0x240404), Kind::Headset);
        assert_eq!(kind_from_cod(0x002540), Kind::Keyboard);
        assert_eq!(kind_from_cod(0x002580), Kind::Mouse);
    }
}
