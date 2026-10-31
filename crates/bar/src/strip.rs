//! The strip window: a layered window placed over the free part of a taskbar.
//!
//! Preferred mode is a child of the taskbar window (`SetParent`), so it moves, hides and
//! auto-hides with the taskbar for free. If Explorer refuses, it falls back to a topmost
//! tool window whose position and visibility are re-checked on every `tick`.

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTF_IDISHWND, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_SETMAXTIPWIDTH, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX, TTTOOLINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowLongPtrW, IsWindow, SendMessageW, SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, UpdateLayeredWindow,
    GWL_STYLE, HWND_TOP, HWND_TOPMOST, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use crate::items::Item;
use crate::render::{Look, StripPainter};
use crate::taskbar::{self, Monitor, Position, Taskbar};

pub const CLASS: PCWSTR = w!("WirelessBatteryTrayStrip");

pub struct Strip {
    pub hwnd: HWND,
    tooltip: HWND,
    painter: Box<dyn StripPainter>,
    monitor: Monitor,
    taskbar: Taskbar,
    parented: bool,
    /// What's currently on screen, to skip redundant repaints.
    painted: Option<(Vec<Item>, Look)>,
    size: (i32, i32),
    pos: (i32, i32),
    shown: bool,
}

impl Strip {
    pub fn create(monitor: Monitor, painter: Box<dyn StripPainter>) -> Result<Self> {
        let taskbar = taskbar::taskbar_for(&monitor).context("no taskbar on that monitor")?;
        // SAFETY: class registered in main; all handles are owned by this struct.
        unsafe {
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                CLASS,
                w!("Wireless Battery Tray"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )?;
            // Child of the taskbar: swap WS_POPUP for WS_CHILD first, as SetParent expects.
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !WS_POPUP.0) | WS_CHILD.0 | WS_CLIPSIBLINGS.0) as isize);
            let parented = SetParent(hwnd, taskbar.hwnd).is_ok();
            if !parented {
                SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
            }
            let tooltip = create_tooltip(hwnd)?;
            Ok(Self { hwnd, tooltip, painter, monitor, taskbar, parented, painted: None, size: (0, 0), pos: (i32::MIN, i32::MIN), shown: false })
        }
    }

    pub fn monitor_name(&self) -> &str {
        &self.monitor.name
    }

    pub fn is_alive(&self) -> bool {
        // SAFETY: plain query.
        unsafe { IsWindow(self.hwnd).as_bool() && IsWindow(self.taskbar.hwnd).as_bool() }
    }

    /// Repaint (if anything changed) and re-place. Cheap enough to call every couple of seconds.
    pub fn update(&mut self, items: &[Item], dark: bool, text_pt: f32, compact: bool, pos: Position, offset: i32) -> Result<()> {
        if let Some(tb) = taskbar::taskbar_for(&self.monitor) {
            self.taskbar = tb;
        }
        let tb = self.taskbar;
        let look = Look { dark, dpi: tb.dpi, height: tb.rect.bottom - tb.rect.top, text_pt, compact };

        if items.is_empty() || (!self.parented && !tb.is_shown(&self.monitor)) {
            self.show(false);
            return Ok(());
        }

        let repaint = self.painted.as_ref().is_none_or(|(i, l)| i != items || *l != look);
        if repaint {
            self.size = self.painter.paint(items, look)?;
            self.set_tooltip(items);
        }

        let (w, h) = self.size;
        let (screen_x, _) = tb.place(w, pos, offset);
        let (x, y) = if self.parented { (screen_x - tb.rect.left, 0) } else { (screen_x, tb.rect.top) };

        // SAFETY: our own window; z-order is re-asserted because Explorer re-stacks its children.
        unsafe {
            let after = if self.parented { HWND_TOP } else { HWND_TOPMOST };
            SetWindowPos(self.hwnd, after, x, y, w, h, SWP_NOACTIVATE)?;
            if repaint || (x, y) != self.pos {
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let size = SIZE { cx: w, cy: h };
                let src = POINT { x: 0, y: 0 };
                UpdateLayeredWindow(
                    self.hwnd,
                    None,
                    None,
                    Some(&size),
                    self.painter.dc(),
                    Some(&src),
                    windows::Win32::Foundation::COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )?;
            }
        }
        self.pos = (x, y);
        if repaint {
            self.painted = Some((items.to_vec(), look));
        }
        self.show(true);
        Ok(())
    }

    fn show(&mut self, on: bool) {
        if on != self.shown {
            // SAFETY: our own window.
            unsafe {
                let _ = ShowWindow(self.hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
            self.shown = on;
        }
    }

    fn set_tooltip(&self, items: &[Item]) {
        let text: Vec<u16> = items.iter().map(|i| i.detail.as_str()).collect::<Vec<_>>().join("\n").encode_utf16().chain(Some(0)).collect();
        let mut ti = tool_info(self.hwnd);
        ti.lpszText = windows::core::PWSTR(text.as_ptr() as *mut _);
        // SAFETY: `text` outlives the synchronous SendMessage; the control copies it.
        unsafe {
            SendMessageW(self.tooltip, TTM_UPDATETIPTEXTW, WPARAM(0), LPARAM(&ti as *const _ as isize));
        }
    }
}

impl Drop for Strip {
    fn drop(&mut self) {
        // SAFETY: destroying our own windows (no-op if Explorer already took them down).
        unsafe {
            let _ = DestroyWindow(self.tooltip);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn tool_info(hwnd: HWND) -> TTTOOLINFOW {
    TTTOOLINFOW {
        cbSize: std::mem::size_of::<TTTOOLINFOW>() as u32,
        uFlags: TTF_IDISHWND | TTF_SUBCLASS,
        hwnd,
        uId: hwnd.0 as usize,
        rect: RECT::default(),
        ..Default::default()
    }
}

fn create_tooltip(owner: HWND) -> Result<HWND> {
    // SAFETY: standard tooltip control setup.
    unsafe {
        let tip = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            PCWSTR::null(),
            WS_POPUP | windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            0,
            0,
            0,
            0,
            owner,
            None,
            None,
            None,
        )?;
        let mut ti = tool_info(owner);
        let empty = [0u16];
        ti.lpszText = windows::core::PWSTR(empty.as_ptr() as *mut _);
        SendMessageW(tip, TTM_ADDTOOLW, WPARAM(0), LPARAM(&ti as *const _ as isize));
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, WPARAM(0), LPARAM(800));
        Ok(tip)
    }
}
