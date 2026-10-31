//! Settings window (`batteries-bar --settings --notify <hwnd>`), a separate short-lived process
//! so the strip never hosts a GUI toolkit. Every change is written to config.toml at once and
//! the bar is told to reload, so the taskbar updates live alongside the preview here.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use eframe::egui::{self, Color32, RichText};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::{load_snapshot, Config, Display, Icon, Reading};
use crate::items::{self, Style};
use crate::taskbar::{self, Monitor};
use crate::{autostart, WM_RELOAD};

pub const TITLE: &str = "Battery bar settings";

pub fn run(notify: Option<isize>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title(TITLE).with_inner_size([900.0, 700.0]).with_min_inner_size([440.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            add_icon_font(&cc.egui_ctx);
            Ok(Box::new(SettingsApp::new(notify)))
        }),
    )
        .map_err(|e| anyhow!("settings window: {e}"))
}

struct SettingsApp {
    cfg: Config,
    saved: Config,
    readings: Vec<Reading>,
    monitors: Vec<Monitor>,
    autostart: bool,
    notify: Option<isize>,
    refreshed: Instant,
    dark: bool,
}

impl SettingsApp {
    fn new(notify: Option<isize>) -> Self {
        let cfg = Config::load();
        Self {
            saved: cfg.clone(),
            cfg,
            readings: load_snapshot(),
            monitors: taskbar::monitors(),
            autostart: autostart::is_enabled(),
            notify,
            refreshed: Instant::now(),
            dark: crate::system_dark(),
        }
    }

    fn apply(&mut self) {
        if self.cfg == self.saved {
            return;
        }
        if self.cfg.save().is_ok() {
            self.saved = self.cfg.clone();
            if let Some(h) = self.notify {
                // SAFETY: posting to the bar's message window; harmless if it's gone.
                let _ = unsafe { PostMessageW(HWND(h as _), WM_RELOAD, WPARAM(0), LPARAM(0)) };
            }
        }
    }

    /// Pick up new readings (the bar rewrites the snapshot after every poll) and newly seen devices.
    fn poll_files(&mut self) {
        if self.refreshed.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.refreshed = Instant::now();
        self.readings = load_snapshot();
        let disk = Config::load();
        for d in disk.devices {
            if !self.cfg.devices.iter().any(|x| x.id == d.id) {
                self.saved.devices.push(d.clone());
                self.cfg.devices.push(d);
            }
        }
    }

    fn preview(&self, ui: &mut egui::Ui) {
        let items = items::build(&self.cfg, &self.readings);
        let (bg, fg) = if self.dark { (Color32::from_rgb(32, 32, 32), Color32::WHITE) } else { (Color32::from_rgb(243, 243, 243), Color32::from_rgb(26, 26, 26)) };
        egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(10, 8)).corner_radius(4.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let size = self.cfg.text_size * 96.0 / 72.0;
            let text = |i: &items::Item| {
                let mut s = String::new();
                if let Some(g) = i.icon {
                    s.push(g);
                    if let Some(b) = i.badge {
                        s.push_str(&b.to_string());
                    }
                    s.push(' ');
                }
                s.push_str(&i.text);
                let color = match i.style {
                    Style::Normal => fg,
                    Style::Low => if self.dark { Color32::from_rgb(255, 107, 107) } else { Color32::from_rgb(196, 43, 28) },
                    Style::Dim => fg.gamma_multiply(0.5),
                };
                RichText::new(s).color(color).size(if self.cfg.compact { size * 0.85 } else { size })
            };
            if items.is_empty() {
                ui.label(RichText::new("(nothing to show)").color(fg.gamma_multiply(0.5)));
            } else if self.cfg.compact {
                ui.horizontal(|ui| {
                    for pair in items.chunks(2) {
                        ui.vertical(|ui| {
                            for i in pair {
                                ui.label(text(i));
                            }
                        });
                        ui.add_space(8.0);
                    }
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    for i in &items {
                        ui.label(text(i)).on_hover_text(&i.detail);
                        ui.add_space(8.0);
                    }
                });
            }
        });
    }

    fn devices(&mut self, ui: &mut egui::Ui) {
        let mut swap: Option<(usize, usize)> = None;
        let n = self.cfg.devices.len();
        let badges = items::badges(&self.cfg);
        egui::Grid::new("devices").num_columns(7).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Show").strong());
            ui.label(RichText::new("Device").strong());
            ui.label(RichText::new("Show as").strong());
            ui.label(RichText::new("Icon").strong());
            ui.label(RichText::new("Label").strong());
            ui.label(RichText::new("Now").strong());
            ui.label("");
            ui.end_row();
            for i in 0..n {
                let d = &mut self.cfg.devices[i];
                ui.checkbox(&mut d.show, "");
                ui.vertical(|ui| {
                    ui.set_min_width(210.0);
                    let badge = badges[i].map(|b| format!("  #{b}")).unwrap_or_default();
                    ui.add(egui::Label::new(format!("{}{badge}", d.name)).extend());
                    ui.add(egui::Label::new(RichText::new(&d.id).small().weak()).extend());
                });
                egui::ComboBox::from_id_salt(("display", i)).width(96.0).selected_text(d.display.name()).show_ui(ui, |ui| {
                    for opt in Display::ALL {
                        ui.selectable_value(&mut d.display, opt, opt.name());
                    }
                });
                let current = d.icon();
                egui::ComboBox::from_id_salt(("icon", i))
                    .width(40.0)
                    .selected_text(RichText::new(current.glyph().to_string()).size(16.0))
                    .show_ui(ui, |ui| {
                        for opt in Icon::ALL {
                            let text = RichText::new(format!("{}  {}", opt.glyph(), opt.name()));
                            if ui.selectable_label(current == opt, text).clicked() {
                                d.icon = (opt != Icon::from_kind(d.kind)).then_some(opt);
                            }
                        }
                    });
                let mut label = d.label.clone().unwrap_or_default();
                let hint = items::auto_label(&d.name);
                if ui.add(egui::TextEdit::singleline(&mut label).hint_text(hint).desired_width(70.0)).changed() {
                    d.label = (!label.trim().is_empty()).then(|| label.trim().to_string());
                }
                ui.label(status(&self.readings, &d.id));
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).clicked() {
                        swap = Some((i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).clicked() {
                        swap = Some((i, i + 1));
                    }
                });
                ui.end_row();
            }
        });
        if let Some((a, b)) = swap {
            self.cfg.devices.swap(a, b);
        }
        if n == 0 {
            ui.label("No devices seen yet. They appear here after the first poll.");
        }
    }
}

/// Use the same Windows icon font as the strip, as a fallback after egui's text fonts, so the
/// preview and the icon picker show the real glyphs.
fn add_icon_font(ctx: &egui::Context) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let Some(bytes) = ["SegoeIcons.ttf", "segmdl2.ttf"].iter().find_map(|f| std::fs::read(format!(r"{windir}\Fonts\{f}")).ok()) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("segoe-icons".into(), std::sync::Arc::new(egui::FontData::from_owned(bytes)));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("segoe-icons".into());
    }
    ctx.set_fonts(fonts);
}

fn status(readings: &[Reading], id: &str) -> String {
    let parts: Vec<String> = readings
        .iter()
        .filter(|r| r.device_id == id)
        .map(|r| match &r.level {
            battery_core::Level::Percent(p) if r.connected == Some(false) => format!("{p}% (last)"),
            battery_core::Level::Percent(p) => format!("{p}%"),
            battery_core::Level::Coarse(s) => s.clone(),
            battery_core::Level::Unavailable(s) => s.clone(),
        })
        .collect();
    if parts.is_empty() {
        "not found".into()
    } else {
        parts.join(", ")
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_files();
        ctx.request_repaint_after(Duration::from_secs(2));

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Preview");
                self.preview(ui);
                ui.add_space(12.0);

                ui.heading("Devices");
                self.devices(ui);
                ui.checkbox(&mut self.cfg.show_offline, "Show devices that are off or asleep (as a dim dash)");
                ui.add_space(12.0);

                ui.heading("Display");
                let current = taskbar::pick(self.cfg.monitor.as_deref()).map(|m| m.name).unwrap_or_default();
                let selected = self.monitors.iter().find(|m| m.name == current).map(|m| m.label()).unwrap_or_default();
                egui::ComboBox::from_label("Show on").selected_text(selected).show_ui(ui, |ui| {
                    for m in &self.monitors {
                        if ui.selectable_label(m.name == current, m.label()).clicked() {
                            self.cfg.monitor = Some(m.name.clone());
                        }
                    }
                });
                ui.add(egui::Slider::new(&mut self.cfg.text_size, 7.0..=14.0).step_by(0.5).text("Text size (pt)"));
                ui.checkbox(&mut self.cfg.compact, "Compact: two rows");
                ui.add_space(12.0);

                ui.heading("Behaviour");
                ui.add(egui::Slider::new(&mut self.cfg.low_threshold, 5..=50).text("Low battery below (%)"));
                ui.add(egui::Slider::new(&mut self.cfg.poll_secs, 15..=600).logarithmic(true).text("Check every (s)"));
                let mut auto = self.autostart;
                if ui.checkbox(&mut auto, "Start with Windows").changed() && autostart::set(auto).is_ok() {
                    self.autostart = auto;
                }
                ui.add_space(8.0);
                ui.label(RichText::new(format!("Config: {}", Config::path().display())).small().weak());
            });
        });

        self.apply();
    }
}
