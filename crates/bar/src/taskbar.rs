//! Monitors, their taskbar windows, and where on a taskbar the strip may go.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowRect, IsWindowVisible, MONITORINFOF_PRIMARY};

#[derive(Debug, Clone)]
pub struct Monitor {
    /// GDI device name, e.g. `\\.\DISPLAY3`; persisted in the config.
    pub name: String,
    pub rect: RECT,
    pub primary: bool,
    pub handle: HMONITOR,
}

impl Monitor {
    /// Menu/settings label: "1: 2560×1440 (primary)".
    pub fn label(&self) -> String {
        let n = self.name.trim_start_matches(r"\\.\DISPLAY");
        let (w, h) = (self.rect.right - self.rect.left, self.rect.bottom - self.rect.top);
        format!("Display {n}: {w}×{h}{}", if self.primary { " (primary)" } else { "" })
    }
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn cb(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the &mut Vec passed below, alive for the whole enumeration.
        let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        // SAFETY: MONITORINFOEXW starts with MONITORINFO and cbSize says which one it is.
        if unsafe { GetMonitorInfoW(mon, &mut info.monitorInfo as *mut MONITORINFO) }.as_bool() {
            let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
            out.push(Monitor {
                name: String::from_utf16_lossy(&info.szDevice[..len]),
                rect: info.monitorInfo.rcMonitor,
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                handle: mon,
            });
        }
        true.into()
    }
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: callback only touches `out` through the LPARAM.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out.sort_by_key(|m| m.name.clone());
    out
}

/// The configured monitor, or the primary one if it's missing/unplugged.
pub fn pick(name: Option<&str>) -> Option<Monitor> {
    let all = monitors();
    name.and_then(|n| all.iter().find(|m| m.name == n).cloned())
        .or_else(|| all.iter().find(|m| m.primary).cloned())
        .or_else(|| all.into_iter().next())
}

#[derive(Debug, Clone, Copy)]
pub struct Taskbar {
    pub hwnd: HWND,
    /// Screen rect, physical pixels.
    pub rect: RECT,
    pub dpi: u32,
    /// Screen x where the free area on the right ends (tray / clock start).
    pub free_right: i32,
}

/// Secondary taskbars have no tray window; their clock and notification bell take about this
/// much on the right (measured on Windows 11 at 100%).
const SECONDARY_CLOCK_DIP: i32 = 120;

pub fn taskbar_for(monitor: &Monitor) -> Option<Taskbar> {
    let mut candidates = Vec::new();
    // SAFETY: plain window lookups.
    unsafe {
        if let Ok(h) = FindWindowExW(None, None, w!("Shell_TrayWnd"), PCWSTR::null()) {
            candidates.push((h, true));
        }
        let mut after = HWND::default();
        while let Ok(h) = FindWindowExW(None, after, w!("Shell_SecondaryTrayWnd"), PCWSTR::null()) {
            candidates.push((h, false));
            after = h;
        }
    }
    let (hwnd, primary) = candidates
        .into_iter()
        // SAFETY: valid window handles from FindWindowExW.
        .find(|(h, _)| unsafe { MonitorFromWindow(*h, MONITOR_DEFAULTTONEAREST) } == monitor.handle)?;

    let mut rect = RECT::default();
    // SAFETY: as above.
    unsafe { GetWindowRect(hwnd, &mut rect).ok()? };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let free_right = if primary {
        // SAFETY: as above.
        let tray = unsafe { FindWindowExW(hwnd, None, w!("TrayNotifyWnd"), PCWSTR::null()) };
        let mut r = RECT::default();
        match tray {
            Ok(t) if unsafe { GetWindowRect(t, &mut r) }.is_ok() && r.right > r.left => r.left,
            _ => rect.right - SECONDARY_CLOCK_DIP * 3 * dpi as i32 / 96,
        }
    } else {
        rect.right - SECONDARY_CLOCK_DIP * dpi as i32 / 96
    };
    Some(Taskbar { hwnd, rect, dpi, free_right })
}

impl Taskbar {
    /// Hidden (auto-hide slid away, full-screen app, Explorer gone).
    pub fn is_shown(&self, monitor: &Monitor) -> bool {
        // SAFETY: plain query.
        if !unsafe { IsWindowVisible(self.hwnd) }.as_bool() {
            return false;
        }
        let mut r = RECT::default();
        if unsafe { GetWindowRect(self.hwnd, &mut r) }.is_err() {
            return false;
        }
        // Auto-hide leaves a 2px sliver on screen.
        let visible_h = r.bottom.min(monitor.rect.bottom) - r.top.max(monitor.rect.top);
        visible_h > 8
    }
}
