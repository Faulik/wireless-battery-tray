//! Wireless Battery Tray: device batteries on the Windows taskbar, with a tray icon.
//! `wireless-battery-tray --settings` opens the settings window (a separate process).

#![windows_subsystem = "windows"]

mod autostart;
mod config;
mod items;
mod poller;
mod render;
mod settings;
mod strip;
mod taskbar;
mod tray;
mod updater;

use std::cell::RefCell;
use std::time::Duration;

use anyhow::Result;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW, KillTimer, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetForegroundWindow, SetTimer, ShowWindow, TranslateMessage, CS_DBLCLKS, MB_ICONWARNING, MB_OK, MSG, SW_RESTORE, WINDOW_EX_STYLE,
    WM_APP, WM_CLOSE, WM_DISPLAYCHANGE, WM_LBUTTONDBLCLK, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPED,
};

use config::{Config, Reading};
use poller::{Command, Poller};
use render::D2dPainter;
use strip::Strip;
use tray::Action;
use updater::Updater;

pub const MSG_CLASS: PCWSTR = w!("WirelessBatteryTrayMsg");
const WM_POLLED: u32 = WM_APP + 1;
const WM_TRAY: u32 = WM_APP + 2;
/// Sent by the settings process after it saved config.toml.
pub const WM_RELOAD: u32 = WM_APP + 3;
/// Sent by the settings process: WPARAM = 1-based index of the device to outline, 0 = none.
pub const WM_HIGHLIGHT: u32 = WM_APP + 4;
const WM_UPDATER: u32 = WM_APP + 5;
/// Sent by the settings process: check for updates now.
pub const WM_CHECK_UPDATES: u32 = WM_APP + 6;
/// Sent by the settings process: install the available update.
pub const WM_INSTALL_UPDATE: u32 = WM_APP + 7;
const TIMER_PLACE: usize = 1;

struct App {
    msg_hwnd: HWND,
    cfg: Config,
    readings: Vec<Reading>,
    strip: Option<Strip>,
    poller: Poller,
    updater: Updater,
    tray: tray_icon::TrayIcon,
    menu: tray_icon::menu::Menu,
    taskbar_created: u32,
    dark: bool,
    highlight: Option<String>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Our own messages that arrived while App was borrowed; posted again once it's free.
    static DEFERRED: RefCell<Vec<(HWND, u32, WPARAM, LPARAM)>> = const { RefCell::new(Vec::new()) };
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--settings") {
        let notify = args.iter().position(|a| a == "--notify").and_then(|i| args.get(i + 1)).and_then(|h| h.parse().ok());
        if let Err(e) = settings::run(notify) {
            eprintln!("wireless-battery-tray: {e:#}");
        }
        return;
    }
    if let Err(e) = run() {
        eprintln!("wireless-battery-tray: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // SAFETY: Win32 setup on the UI thread.
    unsafe {
        // Started by an update: wait for the old version to exit first. Versions up to 0.1.1
        // pass no pid, so also keep retrying the lock for a while.
        let args: Vec<String> = std::env::args().collect();
        let updated = args.iter().position(|a| a == "--updated");
        if let Some(pid) = updated.and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()) {
            updater::wait_for_old(pid);
        }
        let mut tries = 0;
        let _mutex = loop {
            let m = CreateMutexW(None, true, w!("Local\\WirelessBatteryTraySingleInstance"))?;
            if GetLastError() != ERROR_ALREADY_EXISTS {
                break m;
            }
            let _ = CloseHandle(m);
            tries += 1;
            if updated.is_none() || tries > 150 {
                if updated.is_some() {
                    updater::log("updated: another copy kept running; giving up");
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if updated.is_some() {
            updater::log("updated: started");
        }
        updater::clean_old();
        config::migrate();
        autostart::migrate();
        let _ = InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES });
        let instance = GetModuleHandleW(None)?;
        for (class, style) in [(MSG_CLASS, Default::default()), (strip::CLASS, CS_DBLCLKS)] {
            RegisterClassExW(&WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
        }
        // A hidden top-level window (not message-only: it must receive the TaskbarCreated broadcast).
        let msg_hwnd =
            CreateWindowExW(WINDOW_EX_STYLE::default(), MSG_CLASS, w!("Wireless Battery Tray"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, instance, None)?;

        let cfg = Config::load();
        tray::install_handlers(msg_hwnd, WM_TRAY);
        let monitors = taskbar::monitors();
        let current = taskbar::pick(cfg.monitor.as_deref()).map(|m| m.name).unwrap_or_default();
        let menu = tray::build_menu(&cfg, &monitors, &current, autostart::is_enabled(), None)?;
        let tray = tray::create(menu.clone(), "Wireless Battery Tray: reading devices…")?;
        let poller = Poller::start(msg_hwnd, WM_POLLED, cfg.poll_secs);
        let updater = Updater::start(msg_hwnd, WM_UPDATER, cfg.check_updates);
        let mut app = App {
            msg_hwnd,
            cfg,
            readings: Vec::new(),
            strip: None,
            poller,
            updater,
            tray,
            menu,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            dark: system_dark(),
            highlight: None,
        };
        app.ensure_strip();
        APP.with(|a| *a.borrow_mut() = Some(app));
        SetTimer(msg_hwnd, TIMER_PLACE, 2000, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = KillTimer(msg_hwnd, TIMER_PLACE);
        APP.with(|a| a.borrow_mut().take());
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Messages sent re-entrantly while App is borrowed (e.g. by SetWindowPos) go to DefWindowProc.
    // App also stays borrowed while a menu or message box runs its modal loop; our own messages
    // arriving then (poll results, "update installed, quit") must not be lost, so they wait.
    let handled = APP.with(|a| match a.try_borrow_mut() {
        Ok(mut guard) => guard.as_mut().and_then(|app| app.handle(hwnd, msg, wp, lp)),
        Err(_) if (WM_APP..0xC000).contains(&msg) => {
            DEFERRED.with(|d| d.borrow_mut().push((hwnd, msg, wp, lp)));
            Some(LRESULT(0))
        }
        Err(_) => None,
    });
    if APP.with(|a| a.try_borrow_mut().is_ok()) {
        for (h, m, w, l) in DEFERRED.take() {
            // SAFETY: posting is safe for any handle value.
            let _ = unsafe { PostMessageW(h, m, w, l) };
        }
    }
    match handled {
        Some(r) => r,
        // SAFETY: default processing for our own windows.
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

impl App {
    fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let _ = (wp, lp);
        let is_strip = self.strip.as_ref().is_some_and(|s| s.hwnd == hwnd);
        match msg {
            WM_POLLED => {
                if let Some(r) = self.poller.take() {
                    self.readings = r;
                    if self.cfg.remember(&self.readings) {
                        let _ = self.cfg.save();
                        self.rebuild_menu();
                    }
                    self.update_tray_tooltip();
                    self.refresh();
                }
            }
            WM_TRAY => {
                for action in tray::take_actions() {
                    self.on_action(action);
                }
            }
            WM_RELOAD => self.reload(),
            WM_UPDATER => self.on_updater(),
            WM_CHECK_UPDATES => self.updater.send(updater::Command::CheckNow),
            WM_INSTALL_UPDATE => self.on_action(Action::InstallUpdate),
            WM_HIGHLIGHT => {
                self.highlight = wp.0.checked_sub(1).and_then(|i| self.cfg.devices.get(i)).map(|d| d.id.clone());
                self.refresh();
            }
            WM_TIMER if hwnd == self.msg_hwnd => self.refresh(),
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                self.dark = system_dark();
                self.strip = None;
                self.rebuild_menu();
                self.refresh();
            }
            WM_LBUTTONDBLCLK if is_strip => open_settings(self.msg_hwnd),
            WM_RBUTTONUP if is_strip => {
                use tray_icon::menu::ContextMenu;
                // SAFETY: our strip window; the menu is modal on this thread.
                unsafe {
                    self.menu.show_context_menu_for_hwnd(hwnd.0 as isize, None);
                }
            }
            m if m == self.taskbar_created && m != 0 => {
                // Explorer restarted: our child strip died with the old taskbar.
                self.strip = None;
                self.refresh();
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn on_action(&mut self, action: Action) {
        match action {
            Action::Settings => open_settings(self.msg_hwnd),
            Action::Refresh => self.poller.send(Command::RefreshNow),
            Action::Monitor(name) => {
                self.cfg.monitor = Some(name);
                self.strip = None;
                self.save_and_apply();
            }
            Action::Position(p) => {
                self.cfg.position = p;
                self.save_and_apply();
            }
            Action::ToggleDevice(id) => {
                if let Some(d) = self.cfg.devices.iter_mut().find(|d| d.id == id) {
                    d.show = !d.show;
                }
                self.save_and_apply();
            }
            Action::Autostart => {
                let _ = autostart::set(!autostart::is_enabled());
                self.rebuild_menu();
            }
            Action::InstallUpdate => {
                let _ = self.tray.set_tooltip(Some("Wireless Battery Tray: downloading update…"));
                self.updater.send(updater::Command::Install);
            }
            Action::Quit => {
                let title: Vec<u16> = settings::TITLE.encode_utf16().chain(Some(0)).collect();
                // SAFETY: closes our own settings window, if open, and ends the message loop on this thread.
                unsafe {
                    if let Ok(h) = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
                        let _ = PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
                    }
                    PostQuitMessage(0)
                }
            }
        }
    }

    fn on_updater(&mut self) {
        match self.updater.take_installed() {
            // The new version is running and waiting for us to exit.
            Some(Ok(())) => {
                updater::log("quitting for the update");
                // SAFETY: ends the message loop on this thread.
                unsafe { PostQuitMessage(0) }
            }
            Some(Err(e)) => {
                self.update_tray_tooltip();
                let text: Vec<u16> = format!("The update could not be installed:\n{e}").encode_utf16().chain(Some(0)).collect();
                // SAFETY: modal message box owned by our hidden window.
                unsafe { MessageBoxW(self.msg_hwnd, PCWSTR(text.as_ptr()), w!("Wireless Battery Tray"), MB_OK | MB_ICONWARNING) };
            }
            None => {
                tray::set_badge(&self.tray, self.updater.available().is_some());
                self.rebuild_menu();
            }
        }
    }

    fn save_and_apply(&mut self) {
        let _ = self.cfg.save();
        self.rebuild_menu();
        self.refresh();
    }

    fn reload(&mut self) {
        let new = Config::load();
        if new.monitor != self.cfg.monitor {
            self.strip = None;
        }
        if new.poll_secs != self.cfg.poll_secs {
            self.poller.send(Command::SetInterval(new.poll_secs));
        }
        if new.check_updates != self.cfg.check_updates {
            self.updater.send(updater::Command::SetEnabled(new.check_updates));
        }
        self.cfg = new;
        self.rebuild_menu();
        self.refresh();
    }

    fn ensure_strip(&mut self) {
        let alive = self.strip.as_ref().is_some_and(|s| s.is_alive());
        let wanted = taskbar::pick(self.cfg.monitor.as_deref());
        let right_monitor = match (&self.strip, &wanted) {
            (Some(s), Some(m)) => s.monitor_name() == m.name,
            _ => false,
        };
        if alive && right_monitor {
            return;
        }
        self.strip = None;
        if let Some(m) = wanted {
            match D2dPainter::new().and_then(|p| Strip::create(m, Box::new(p))) {
                Ok(s) => self.strip = Some(s),
                Err(e) => eprintln!("strip: {e:#}"),
            }
        }
    }

    fn refresh(&mut self) {
        self.ensure_strip();
        let mut items = items::build(&self.cfg, &self.readings);
        for i in &mut items {
            i.highlight = self.highlight.as_ref() == Some(&i.device_id);
        }
        if let Some(s) = self.strip.as_mut() {
            if let Err(e) = s.update(&items, self.dark, self.cfg.text_size, self.cfg.compact, self.cfg.position, self.cfg.offset) {
                eprintln!("strip update: {e:#}");
                self.strip = None;
            }
        }
    }

    fn rebuild_menu(&mut self) {
        let monitors = taskbar::monitors();
        let current = taskbar::pick(self.cfg.monitor.as_deref()).map(|m| m.name).unwrap_or_default();
        let update = self.updater.available().map(|r| r.version);
        if let Ok(menu) = tray::build_menu(&self.cfg, &monitors, &current, autostart::is_enabled(), update.as_deref()) {
            self.tray.set_menu(Some(Box::new(menu.clone())));
            self.menu = menu;
        }
    }

    fn update_tray_tooltip(&self) {
        let items = items::build(&self.cfg, &self.readings);
        let text = if items.is_empty() {
            "Wireless Battery Tray: no devices".to_string()
        } else {
            items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join("  ")
        };
        let _ = self.tray.set_tooltip(Some(text));
    }
}

/// Windows "system" (taskbar) theme: SystemUsesLightTheme = 0 means dark.
pub fn system_dark() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    // SAFETY: DWORD out-buffer of the declared size.
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    err.is_err() || value == 0
}

fn open_settings(msg_hwnd: HWND) {
    // Already open: bring it forward instead of starting a second one.
    let title: Vec<u16> = settings::TITLE.encode_utf16().chain(Some(0)).collect();
    // SAFETY: plain window lookup/activation.
    unsafe {
        if let Ok(h) = FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
            return;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).args(["--settings", "--notify", &(msg_hwnd.0 as isize).to_string()]).spawn();
    }
}
