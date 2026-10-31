//! Drawing the strip into a premultiplied 32-bit bitmap for `UpdateLayeredWindow`.
//!
//! `StripPainter` is the seam for swapping the renderer (e.g. GPUI later); the window and
//! placement code only needs a memory DC with the finished frame.

use anyhow::Result;
use windows::core::w;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_TEXT_METRICS,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};

use crate::items::{Item, Style};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub dark: bool,
    pub dpi: u32,
    /// Strip height in pixels (the taskbar's height).
    pub height: i32,
    pub text_pt: f32,
    pub compact: bool,
}

pub trait StripPainter {
    /// Render `items`; returns the frame size in pixels. The frame stays in `dc()` until the next call.
    fn paint(&mut self, items: &[Item], look: Look) -> Result<(i32, i32)>;
    fn dc(&self) -> HDC;
}

pub struct D2dPainter {
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: ID2D1DCRenderTarget,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    size: (i32, i32),
}

impl D2dPainter {
    pub fn new() -> Result<Self> {
        // SAFETY: plain factory/DC creation; every handle is released in Drop.
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                // 96 DPI so one DIP is one pixel; we scale fonts ourselves per monitor.
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let target = d2d.CreateDCRenderTarget(&props)?;
            target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dc = CreateCompatibleDC(None);
            Ok(Self { d2d, dwrite, target, dc, bitmap: HBITMAP::default(), old: HGDIOBJ::default(), size: (0, 0) })
        }
    }

    fn format(&self, px: f32, bold: bool) -> Result<IDWriteTextFormat> {
        let weight = if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
        // SAFETY: valid static strings.
        Ok(unsafe {
            self.dwrite.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                px,
                w!("en-us"),
            )?
        })
    }

    fn layout(&self, text: &str, format: &IDWriteTextFormat) -> Result<(IDWriteTextLayout, f32, f32)> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        // SAFETY: `wide` outlives the call; DirectWrite copies the text.
        unsafe {
            let layout = self.dwrite.CreateTextLayout(&wide, format, 10_000.0, 1_000.0)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            Ok((layout, m.widthIncludingTrailingWhitespace, m.height))
        }
    }

    fn ensure_bitmap(&mut self, w: i32, h: i32) -> Result<()> {
        if self.size == (w, h) {
            return Ok(());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the DIB is selected into our own DC and the previous one is freed.
        unsafe {
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(self.dc, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let prev = SelectObject(self.dc, bitmap);
            if self.bitmap.is_invalid() {
                self.old = prev;
            } else {
                let _ = DeleteObject(self.bitmap);
            }
            self.bitmap = bitmap;
            self.target.BindDC(self.dc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
        }
        self.size = (w, h);
        Ok(())
    }
}

impl StripPainter for D2dPainter {
    fn paint(&mut self, items: &[Item], look: Look) -> Result<(i32, i32)> {
        let scale = look.dpi as f32 / 96.0;
        let rows = if look.compact && items.len() > 1 { 2 } else { 1 };
        let pt = if rows == 2 { look.text_pt * 0.85 } else { look.text_pt };
        let px = pt * look.dpi as f32 / 72.0;
        let format = self.format(px, false)?;
        let gap = 12.0 * scale;
        let pad = 6.0 * scale;

        let laid: Vec<_> = items.iter().map(|i| self.layout(&i.text, &format)).collect::<Result<_>>()?;
        // Column layout: in compact mode items fill top then bottom row, column by column.
        let mut columns: Vec<(f32, Vec<usize>)> = Vec::new();
        for (i, (_, w, _)) in laid.iter().enumerate() {
            if i % rows == 0 {
                columns.push((0.0, Vec::new()));
            }
            let col = columns.last_mut().unwrap();
            col.0 = col.0.max(*w);
            col.1.push(i);
        }
        let content: f32 = columns.iter().map(|c| c.0).sum::<f32>() + gap * columns.len().saturating_sub(1) as f32;
        let width = ((content + 2.0 * pad).ceil() as i32).max(1);
        let height = look.height.max(1);
        self.ensure_bitmap(width, height)?;

        let row_h = height as f32 / rows as f32;
        // SAFETY: drawing on our bound DC render target.
        unsafe {
            self.target.BeginDraw();
            // Alpha 1/255 everywhere so the whole strip is hit-testable (hover tooltip) but invisible.
            self.target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 / 255.0 }));
            let mut x = pad;
            for (col_w, idxs) in &columns {
                for (row, &i) in idxs.iter().enumerate() {
                    let (layout, _, h) = &laid[i];
                    let brush = self.target.CreateSolidColorBrush(&color(items[i].style, look.dark), None)?;
                    let y = row as f32 * row_h + (row_h - h) / 2.0;
                    self.target.DrawTextLayout(
                        D2D_POINT_2F { x: x.round(), y: y.round() },
                        layout,
                        &brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
                x += col_w + gap;
            }
            self.target.EndDraw(None, None)?;
        }
        Ok((width, height))
    }

    fn dc(&self) -> HDC {
        self.dc
    }
}

impl Drop for D2dPainter {
    fn drop(&mut self) {
        // SAFETY: restoring the original bitmap before freeing ours and the DC.
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.dc, self.old);
                let _ = DeleteObject(self.bitmap);
            }
            let _ = DeleteDC(self.dc);
        }
        let _ = &self.d2d;
    }
}

fn color(style: Style, dark: bool) -> D2D1_COLOR_F {
    let (r, g, b, a) = match (style, dark) {
        (Style::Normal, true) => (1.0, 1.0, 1.0, 1.0),
        (Style::Normal, false) => (0.1, 0.1, 0.1, 1.0),
        (Style::Low, true) => (1.0, 0.42, 0.42, 1.0),
        (Style::Low, false) => (0.77, 0.17, 0.11, 1.0),
        (Style::Dim, true) => (1.0, 1.0, 1.0, 0.5),
        (Style::Dim, false) => (0.1, 0.1, 0.1, 0.5),
    };
    D2D1_COLOR_F { r, g, b, a }
}

