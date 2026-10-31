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
use crate::taskbar::{self, Monitor, Position};
use crate::updater::{self, Status};
use crate::{autostart, WM_CHECK_UPDATES, WM_HIGHLIGHT, WM_INSTALL_UPDATE, WM_RELOAD};

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
    highlight: Option<String>,
    update: Status,
    /// Set while waiting for the bar to answer "Check now" or "Install".
    update_busy: Option<&'static str>,
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
            highlight: None,
            update: updater::load_status(),
            update_busy: None,
        }
    }

    /// Tell the bar something; false when it isn't running (or we weren't started by it).
    fn post(&self, msg: u32, wp: usize) -> bool {
        // SAFETY: posting to the bar's message window; harmless if it's gone.
        self.notify.is_some_and(|h| unsafe { PostMessageW(HWND(h as _), msg, WPARAM(wp), LPARAM(0)) }.is_ok())
    }

    fn apply(&mut self) {
        if self.cfg == self.saved {
            return;
        }
        if self.cfg.save().is_ok() {
            self.saved = self.cfg.clone();
            self.post(WM_RELOAD, 0);
        }
    }

    fn updates(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("Version {}", updater::CURRENT));
        ui.checkbox(&mut self.cfg.check_updates, "Check for updates automatically");
        ui.horizontal(|ui| {
            let idle = self.update_busy.is_none() && self.notify.is_some();
            if ui.add_enabled(idle, egui::Button::new("Check now")).clicked() && self.post(WM_CHECK_UPDATES, 0) {
                self.update_busy = Some("Checking…");
            }
            if let Some(r) = self.update.newer.clone() {
                if ui.add_enabled(idle, egui::Button::new(format!("Update to v{}", r.version))).clicked() && self.post(WM_INSTALL_UPDATE, 0) {
                    self.update_busy = Some("Downloading and restarting the bar…");
                }
                ui.hyperlink_to("What's new", &r.page);
            }
        });
        let note = match (&self.update_busy, &self.update) {
            (Some(busy), _) => busy.to_string(),
            (None, Status { checked: None, .. }) => "Not checked yet.".into(),
            (None, Status { error: Some(e), .. }) => e.clone(),
            (None, Status { newer: Some(r), .. }) => format!("Version {} is available.", r.version),
            (None, Status { checked: Some(t), .. }) => format!("Up to date (checked {}).", ago(*t)),
        };
        ui.label(RichText::new(note).small().weak());
        if self.notify.is_none() {
            ui.label(RichText::new("Open settings from the bar to check for updates.").small().weak());
        }
    }

    /// Pick up new readings (the bar rewrites the snapshot after every poll) and newly seen devices.
    fn poll_files(&mut self) {
        if self.refreshed.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.refreshed = Instant::now();
        self.readings = load_snapshot();
        let update = updater::load_status();
        if update != self.update {
            self.update = update;
            self.update_busy = None;
        }
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
        let (bg, fg) =
            if self.dark { (Color32::from_rgb(32, 32, 32), Color32::WHITE) } else { (Color32::from_rgb(243, 243, 243), Color32::from_rgb(26, 26, 26)) };
        egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(10, 8)).corner_radius(4.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let size = self.cfg.text_size * 96.0 / 72.0;
            let text = |i: &items::Item| {
                let mut s = String::new();
                if let Some(g) = i.icon {
                    s.push(g);
                    s.push(' ');
                }
                if let Some(t) = &i.tag {
                    s.push_str(t);
                    s.push(' ');
                }
                s.push_str(&i.text);
                let color = match i.style {
                    Style::Normal => fg,
                    Style::Low => {
                        if self.dark {
                            Color32::from_rgb(255, 107, 107)
                        } else {
                            Color32::from_rgb(196, 43, 28)
                        }
                    }
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
        let mut hovered: Option<usize> = None;
        let n = self.cfg.devices.len();
        let tags = items::tags(&self.cfg, &self.readings);
        egui::Grid::new("devices").num_columns(7).striped(true).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("Show").strong());
            ui.label(RichText::new("Device").strong());
            ui.label(RichText::new("Show as").strong());
            ui.label(RichText::new("Icon").strong());
            ui.label(RichText::new("Short tag").strong());
            ui.label(RichText::new("Now").strong());
            ui.label("");
            ui.end_row();
            // Indexes devices, readings-derived tags and the swap buttons together.
            #[allow(clippy::needless_range_loop)]
            for i in 0..n {
                let connection =
                    self.readings.iter().find(|r| r.device_id == self.cfg.devices[i].id).map(|r| items::connection(&r.source, &r.device_id)).unwrap_or("");
                let d = &mut self.cfg.devices[i];
                let mut row = ui.checkbox(&mut d.show, "");
                let name = ui.vertical(|ui| {
                    ui.set_min_width(210.0);
                    ui.add(egui::Label::new(&d.name).extend());
                    if !connection.is_empty() {
                        ui.add(egui::Label::new(RichText::new(connection).small().weak()).extend());
                    }
                });
                row = row.union(name.response);
                let display = egui::ComboBox::from_id_salt(("display", i)).width(96.0).selected_text(d.display.name()).show_ui(ui, |ui| {
                    for opt in Display::ALL {
                        ui.selectable_value(&mut d.display, opt, opt.name());
                    }
                });
                row = row.union(display.response.on_hover_text("Auto: just the icon, plus the short tag when another shown device has the same icon"));
                let current = d.icon();
                let icon = egui::ComboBox::from_id_salt(("icon", i)).width(40.0).selected_text(RichText::new(current.glyph().to_string()).size(16.0)).show_ui(
                    ui,
                    |ui| {
                        for opt in Icon::ALL {
                            let text = RichText::new(format!("{}  {}", opt.glyph(), opt.name()));
                            if ui.selectable_label(current == opt, text).clicked() {
                                d.icon = (opt != Icon::from_kind(d.kind)).then_some(opt);
                            }
                        }
                    },
                );
                row = row.union(icon.response);
                let mut label = d.label.clone().unwrap_or_default();
                let hint = items::auto_label(&d.name);
                let edit = ui.add(egui::TextEdit::singleline(&mut label).hint_text(hint).desired_width(70.0));
                if edit.changed() {
                    d.label = (!label.trim().is_empty()).then(|| label.trim().to_string());
                }
                row = row.union(edit);
                let now = ui.horizontal(|ui| {
                    ui.label(status(&self.readings, &d.id));
                    if tags[i].as_ref().is_some_and(|t| t.clashed) {
                        ui.label(RichText::new("⚠ same tag as another device").color(Color32::from_rgb(230, 160, 40)))
                            .on_hover_text("Give one of them its own short tag so you can tell them apart.");
                    }
                });
                row = row.union(now.response);
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).clicked() {
                        swap = Some((i, i - 1));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).clicked() {
                        swap = Some((i, i + 1));
                    }
                });
                if row.hovered() {
                    hovered = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some((a, b)) = swap {
            self.cfg.devices.swap(a, b);
        }
        if n == 0 {
            ui.label("No devices seen yet. They appear here after the first poll.");
        }
        self.set_highlight(hovered.map(|i| self.cfg.devices[i].id.clone()));

        egui::CollapsingHeader::new("Details").show(ui, |ui| {
            egui::Grid::new("ids").num_columns(3).spacing([16.0, 2.0]).show(ui, |ui| {
                ui.label(RichText::new("Device").strong());
                ui.label(RichText::new("Detected as").strong());
                ui.label(RichText::new("Id").strong());
                ui.end_row();
                for d in &self.cfg.devices {
                    let reading = self.readings.iter().find(|r| r.device_id == d.id);
                    ui.label(&d.name);
                    ui.label(match reading {
                        Some(r) => format!("{:?} (from {})", r.kind, r.kind_source.describe()),
                        None => format!("{:?}", d.kind),
                    });
                    ui.label(RichText::new(&d.id).monospace().weak());
                    ui.end_row();
                }
            });
        });
    }

    /// Why the strip can't go where the position setting asks, if it can't (checked with a typical strip width).
    fn placement_note(&self) -> Option<&'static str> {
        let monitor = taskbar::pick(self.cfg.monitor.as_deref())?;
        let tb = taskbar::taskbar_for(&monitor)?;
        tb.place(240 * tb.dpi as i32 / 96, self.cfg.position, self.cfg.offset).1
    }

    /// Outline the hovered device on the live strip (index into config order; 0 = none).
    fn set_highlight(&mut self, id: Option<String>) {
        if id == self.highlight {
            return;
        }
        // The bar addresses devices by position in its config, so only send once saved.
        let index = id.as_ref().and_then(|id| self.saved.devices.iter().position(|d| &d.id == id)).map_or(0, |i| i + 1);
        self.post(WM_HIGHLIGHT, index);
        self.highlight = id;
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

fn ago(unix_secs: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(unix_secs) / 60 {
        0 => "just now".into(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 48 * 60 => format!("{} h ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

fn status(readings: &[Reading], id: &str) -> String {
    let parts: Vec<String> = readings
        .iter()
        .filter(|r| r.device_id == id)
        .map(|r| match &r.level {
            battery_core::Level::Percent(p) if r.connected == Some(false) => format!("disconnected (last {p}%)"),
            battery_core::Level::Percent(p) => format!("{p}%"),
            battery_core::Level::Coarse(s) => s.clone(),
            battery_core::Level::Unavailable(s) => format!("disconnected ({s})"),
        })
        .collect();
    if parts.is_empty() {
        "disconnected".into()
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
                ui.checkbox(&mut self.cfg.show_offline, "Show disconnected devices (dimmed)");
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
                egui::ComboBox::from_label("Position").selected_text(self.cfg.position.name()).show_ui(ui, |ui| {
                    for p in Position::ALL {
                        ui.selectable_value(&mut self.cfg.position, p, p.name());
                    }
                });
                ui.add(egui::Slider::new(&mut self.cfg.offset, 0..=600).text("Offset (px)"));
                if let Some(why) = self.placement_note() {
                    ui.label(RichText::new(format!("⚠ Shown on the right instead: {why}.")).color(Color32::from_rgb(230, 160, 40)));
                }
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
                ui.add_space(12.0);

                ui.heading("Updates");
                self.updates(ui);
                ui.add_space(8.0);
                ui.label(RichText::new(format!("Config: {}", Config::path().display())).small().weak());
            });
        });

        self.apply();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.set_highlight(None);
    }
}
