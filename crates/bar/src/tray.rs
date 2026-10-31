//! Tray icon and its menu. Menu/tray events arrive on tray-icon's handler (any thread) and are
//! forwarded to the UI thread as posted messages.

use std::sync::Mutex;

use anyhow::Result;
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::Config;
use crate::items;
use crate::taskbar::{Monitor, Position};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Settings,
    Refresh,
    Monitor(String),
    Position(Position),
    ToggleDevice(String),
    Autostart,
    InstallUpdate,
    Quit,
}

static PENDING: Mutex<Vec<Action>> = Mutex::new(Vec::new());

pub fn take_actions() -> Vec<Action> {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

/// Route tray-icon's events to `hwnd` as `msg`.
pub fn install_handlers(hwnd: HWND, msg: u32) {
    let h = hwnd.0 as isize;
    let post = move |a: Action| {
        PENDING.lock().unwrap().push(a);
        // SAFETY: posting is safe for any handle value.
        let _ = unsafe { PostMessageW(HWND(h as _), msg, WPARAM(0), LPARAM(0)) };
    };
    let post2 = post;
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if let Some(a) = parse(e.id.as_ref()) {
            post(a);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if matches!(e, TrayIconEvent::DoubleClick { .. }) {
            post2(Action::Settings);
        }
    }));
}

fn parse(id: &str) -> Option<Action> {
    Some(match id {
        "settings" => Action::Settings,
        "refresh" => Action::Refresh,
        "autostart" => Action::Autostart,
        "update" => Action::InstallUpdate,
        "quit" => Action::Quit,
        _ => {
            if let Some(p) = id.strip_prefix("position:") {
                Action::Position(Position::ALL.into_iter().find(|x| format!("{x:?}") == p)?)
            } else if let Some(m) = id.strip_prefix("monitor:") {
                Action::Monitor(m.to_string())
            } else {
                Action::ToggleDevice(id.strip_prefix("device:")?.to_string())
            }
        }
    })
}

/// `update`: version of a newer release, offered at the top of the menu.
pub fn build_menu(cfg: &Config, monitors: &[Monitor], current_monitor: &str, autostart: bool, update: Option<&str>) -> Result<Menu> {
    let monitor_items: Vec<CheckMenuItem> =
        monitors.iter().map(|m| CheckMenuItem::with_id(format!("monitor:{}", m.name), m.label(), true, m.name == current_monitor, None)).collect();
    let device_items: Vec<CheckMenuItem> = cfg
        .devices
        .iter()
        .map(|d| CheckMenuItem::with_id(format!("device:{}", d.id), format!("{} ({})", d.name, items::label(d)), true, d.show, None))
        .collect();
    fn refs(v: &[CheckMenuItem]) -> Vec<&dyn IsMenuItem> {
        v.iter().map(|i| i as &dyn IsMenuItem).collect()
    }

    let monitor_menu = Submenu::with_items("Show on", true, &refs(&monitor_items))?;
    let position_items: Vec<CheckMenuItem> =
        Position::ALL.iter().map(|p| CheckMenuItem::with_id(format!("position:{p:?}"), p.name(), true, *p == cfg.position, None)).collect();
    let position_menu = Submenu::with_items("Position", true, &refs(&position_items))?;
    let device_menu = Submenu::with_items("Devices", !device_items.is_empty(), &refs(&device_items))?;
    let menu = Menu::with_items(&[
        &MenuItem::with_id("settings", "Settings…", true, None),
        &MenuItem::with_id("refresh", "Refresh now", true, None),
        &PredefinedMenuItem::separator(),
        &monitor_menu,
        &position_menu,
        &device_menu,
        &CheckMenuItem::with_id("autostart", "Start with Windows", true, autostart, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("quit", "Quit", true, None),
    ])?;
    if let Some(v) = update {
        menu.prepend_items(&[&MenuItem::with_id("update", format!("Update to v{v}"), true, None), &PredefinedMenuItem::separator()])?;
    }
    Ok(menu)
}

pub fn create(menu: Menu, tooltip: &str) -> Result<TrayIcon> {
    Ok(TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(tooltip).with_icon(icon(false)?).build()?)
}

/// Show or clear the "update available" dot on the tray icon.
pub fn set_badge(tray: &TrayIcon, badge: bool) {
    if let Ok(i) = icon(badge) {
        let _ = tray.set_icon(Some(i));
    }
}

/// A small white battery outline, drawn in code so there's no asset to ship; `badge` adds a
/// blue dot in the top-right corner.
fn icon(badge: bool) -> Result<Icon> {
    const S: usize = 32;
    let mut px = vec![0u8; S * S * 4];
    if badge {
        for y in 0..10 {
            for x in 22..32 {
                let (dx, dy) = (x as i32 - 27, y as i32 - 4);
                if dx * dx + dy * dy <= 20 {
                    let i = (y * S + x) * 4;
                    px[i..i + 4].copy_from_slice(&[0, 120, 215, 255]);
                }
            }
        }
    }
    let mut set = |x: usize, y: usize| {
        let i = (y * S + x) * 4;
        px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
    };
    for x in 3..27 {
        for y in [9, 10, 21, 22] {
            set(x, y);
        }
    }
    for y in 9..23 {
        for x in [3, 4, 25, 26] {
            set(x, y);
        }
    }
    for x in 27..30 {
        for y in 13..19 {
            set(x, y);
        }
    }
    for x in 7..16 {
        for y in 13..19 {
            set(x, y);
        }
    }
    Ok(Icon::from_rgba(px, S as u32, S as u32)?)
}
