//! Settings window (`wireless-battery-tray --settings --notify <hwnd>`), a separate short-lived process
//! so the strip never hosts a GUI toolkit. Every change is written to config.toml at once and
//! the bar is told to reload, so the taskbar updates live alongside the preview here.

use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::config::{load_snapshot, Config, Display, Icon, Reading};
use crate::items::{self, Style};
use crate::taskbar::{self, Monitor, Position};
use crate::updater::{self, Status};
use crate::{autostart, WM_CHECK_UPDATES, WM_HIGHLIGHT, WM_INSTALL_UPDATE, WM_RELOAD};

pub const TITLE: &str = "Wireless Battery Tray settings";

/// Set in eframe's storage once the window has been fitted to its content; from then on the
/// size and position the user leaves it at are restored instead.
const FITTED_KEY: &str = "fitted";
const ORANGE: Color32 = Color32::from_rgb(230, 160, 40);

pub fn run(notify: Option<isize>) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_app_id("wireless-battery-tray")
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([560.0, 420.0]),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |cc| {
            add_icon_font(&cc.egui_ctx);
            style(&cc.egui_ctx);
            let fit = cc.storage.and_then(|s| s.get_string(FITTED_KEY)).is_none();
            let tab = cc.storage.and_then(|s| s.get_string(TAB_KEY)).and_then(|t| Tab::ALL.into_iter().find(|x| x.name() == t)).unwrap_or(Tab::Devices);
            Ok(Box::new(SettingsApp::new(notify, fit, tab)))
        }),
    )
    .map_err(|e| anyhow!("settings window: {e}"))
}

/// Windows 11-ish: a little more breathing room and rounded controls.
fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(8.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        let w = &mut s.visuals.widgets;
        for v in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            v.corner_radius = 4.0.into();
        }
    });
}

const TAB_KEY: &str = "tab";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Devices,
    Display,
    Behaviour,
    About,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Devices, Tab::Display, Tab::Behaviour, Tab::About];
    fn name(self) -> &'static str {
        match self {
            Tab::Devices => "Devices",
            Tab::Display => "Display",
            Tab::Behaviour => "Behaviour",
            Tab::About => "About & updates",
        }
    }
}

/// Width of the label column in the settings grids, so controls line up across tabs.
const LABEL_WIDTH: f32 = 170.0;
const CONTROL_WIDTH: f32 = 300.0;

fn settings_grid(ui: &mut egui::Ui, id: &str, rows: impl FnOnce(&mut egui::Ui)) {
    ui.spacing_mut().slider_width = CONTROL_WIDTH - 70.0;
    egui::Grid::new(id).num_columns(2).min_col_width(LABEL_WIDTH).spacing([16.0, 12.0]).show(ui, rows);
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
    /// Fit the window height to the content on the first frames (first run only).
    fit: bool,
    frames: u32,
    tab: Tab,
}

impl SettingsApp {
    fn new(notify: Option<isize>, fit: bool, tab: Tab) -> Self {
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
            fit,
            frames: 0,
            tab,
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

    fn about(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Wireless Battery Tray").strong().size(16.0));
        ui.label(format!("Version {}", updater::CURRENT));
        ui.horizontal(|ui| {
            ui.hyperlink_to("Website", "https://battery.faulcloud.work/");
            ui.label("·");
            ui.hyperlink_to("GitHub", "https://github.com/Faulik/wireless-battery-tray");
        });
        ui.add_space(12.0);
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
        ui.label(RichText::new(note).weak());
        if self.notify.is_none() {
            ui.label(RichText::new("Open settings from the tray icon to check for updates.").weak());
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Config: {}", Config::path().display())).weak());
            if ui.small_button("Open folder").clicked() {
                let _ = std::process::Command::new("explorer").arg(crate::config::dir()).spawn();
            }
        });
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
        let (text, weak) = (ui.visuals().text_color(), ui.visuals().weak_text_color());
        let devices = &mut self.cfg.devices;
        let readings = &self.readings;
        // The device column is as wide as the longest name + connection, as long as that leaves
        // the status column some room; names only get cut (full on hover) in a narrow window.
        let measure = |s: &str, size: f32| ui.fonts(|f| f.layout_no_wrap(s.to_owned(), egui::FontId::proportional(size), text).size().x);
        let longest = devices
            .iter()
            .map(|d| {
                let conn = readings.iter().find(|r| r.device_id == d.id).map(|r| items::connection(&r.source, &r.device_id)).unwrap_or("");
                measure(&d.name, 14.0) + 8.0 + measure(conn, 11.5)
            })
            .fold(0.0, f32::max);
        let fixed = 40.0 + 112.0 + 60.0 + 110.0 + 58.0 + 7.0 * ui.spacing().item_spacing.x;
        let device_width = (longest + 12.0).min(ui.available_width() - fixed - 180.0).max(180.0);
        TableBuilder::new(ui)
            .id_salt("devices")
            .striped(true)
            .vscroll(false)
            .sense(egui::Sense::hover())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(40.0))
            .column(Column::exact(device_width).clip(true))
            .column(Column::exact(112.0))
            .column(Column::exact(60.0))
            .column(Column::exact(110.0))
            .column(Column::remainder().at_least(150.0).clip(true))
            .column(Column::exact(58.0))
            .header(24.0, |mut h| {
                for title in ["Show", "Device", "Show as", "Icon", "Short tag", "Now", ""] {
                    h.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|mut body| {
                for i in 0..n {
                    body.row(30.0, |mut row| {
                        let d = &mut devices[i];
                        let connection = readings.iter().find(|r| r.device_id == d.id).map(|r| items::connection(&r.source, &r.device_id)).unwrap_or("");
                        let mut cells = Vec::with_capacity(7);
                        cells.push(row.col(|ui| {
                            ui.checkbox(&mut d.show, "");
                        }));
                        cells.push(row.col(|ui| {
                            // The connection always shows; the name gives way if the column is too narrow.
                            let conn = RichText::new(connection).size(11.5).color(weak);
                            let conn_width = ui.fonts(|f| f.layout_no_wrap(connection.to_owned(), egui::FontId::proportional(11.5), weak).size().x);
                            let name_width = (ui.available_width() - conn_width - 8.0).max(40.0);
                            let full = if connection.is_empty() { d.name.clone() } else { format!("{} ({connection})", d.name) };
                            ui.allocate_ui(egui::vec2(name_width, ui.available_height()), |ui| {
                                ui.add(egui::Label::new(RichText::new(&d.name).color(text)).truncate()).on_hover_text(&full);
                            });
                            ui.label(conn);
                        }));
                        cells.push(row.col(|ui| {
                            egui::ComboBox::from_id_salt(("display", i))
                                .width(ui.available_width())
                                .selected_text(d.display.name())
                                .show_ui(ui, |ui| {
                                    for opt in Display::ALL {
                                        ui.selectable_value(&mut d.display, opt, opt.name());
                                    }
                                })
                                .response
                                .on_hover_text("Auto: just the icon, plus the short tag when another shown device has the same icon");
                        }));
                        cells.push(row.col(|ui| {
                            let current = d.icon();
                            egui::ComboBox::from_id_salt(("icon", i))
                                .width(ui.available_width())
                                .selected_text(RichText::new(current.glyph().to_string()).size(16.0))
                                .show_ui(ui, |ui| {
                                    for opt in Icon::ALL {
                                        let text = RichText::new(format!("{}  {}", opt.glyph(), opt.name()));
                                        if ui.selectable_label(current == opt, text).clicked() {
                                            d.icon = (opt != Icon::from_kind(d.kind)).then_some(opt);
                                        }
                                    }
                                });
                        }));
                        cells.push(row.col(|ui| {
                            let mut label = d.label.clone().unwrap_or_default();
                            let hint = items::auto_label(&d.name);
                            let edit = egui::TextEdit::singleline(&mut label).hint_text(hint).desired_width(ui.available_width());
                            if ui.add(edit).changed() {
                                d.label = (!label.trim().is_empty()).then(|| label.trim().to_string());
                            }
                        }));
                        cells.push(row.col(|ui| {
                            if tags[i].as_ref().is_some_and(|t| t.clashed) {
                                ui.label(RichText::new("⚠ same tag").color(ORANGE))
                                    .on_hover_text("Another shown device has the same tag. Give one of them its own short tag so you can tell them apart.");
                            }
                            let now = status(readings, &d.id);
                            ui.add(egui::Label::new(&now).truncate()).on_hover_text(&now);
                        }));
                        cells.push(row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if ui.add_enabled(i > 0, egui::Button::new("⏶").small()).on_hover_text("Move left on the strip").clicked() {
                                swap = Some((i, i - 1));
                            }
                            if ui.add_enabled(i + 1 < n, egui::Button::new("⏷").small()).on_hover_text("Move right on the strip").clicked() {
                                swap = Some((i, i + 1));
                            }
                        }));
                        if cells.iter().any(|(_, r)| r.hovered()) {
                            hovered = Some(i);
                        }
                    });
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

    fn display(&mut self, ui: &mut egui::Ui) {
        let control = CONTROL_WIDTH;
        settings_grid(ui, "display", |ui| {
            ui.label("Show on");
            let current = taskbar::pick(self.cfg.monitor.as_deref()).map(|m| m.name).unwrap_or_default();
            let selected = self.monitors.iter().find(|m| m.name == current).map(|m| m.label()).unwrap_or_default();
            egui::ComboBox::from_id_salt("monitor").width(control).selected_text(selected).show_ui(ui, |ui| {
                for m in &self.monitors {
                    if ui.selectable_label(m.name == current, m.label()).clicked() {
                        self.cfg.monitor = Some(m.name.clone());
                    }
                }
            });
            ui.end_row();
            ui.label("Position");
            egui::ComboBox::from_id_salt("position").width(control).selected_text(self.cfg.position.name()).show_ui(ui, |ui| {
                for p in Position::ALL {
                    ui.selectable_value(&mut self.cfg.position, p, p.name());
                }
            });
            ui.end_row();
            ui.label("Offset");
            ui.add(egui::Slider::new(&mut self.cfg.offset, 0..=600).suffix(" px"));
            ui.end_row();
            ui.label("Text size");
            ui.add(egui::Slider::new(&mut self.cfg.text_size, 7.0..=14.0).step_by(0.5).suffix(" pt"));
            ui.end_row();
            ui.label("Layout");
            ui.checkbox(&mut self.cfg.compact, "Compact: two rows");
            ui.end_row();
        });
        if let Some(why) = self.placement_note() {
            ui.add_space(8.0);
            ui.label(RichText::new(format!("⚠ Shown on the right instead: {why}.")).color(ORANGE));
        }
    }

    fn behaviour(&mut self, ui: &mut egui::Ui) {
        settings_grid(ui, "behaviour", |ui| {
            ui.label("Low battery below");
            ui.add(egui::Slider::new(&mut self.cfg.low_threshold, 5..=50).suffix(" %"));
            ui.end_row();
            ui.label("Check batteries every");
            ui.add(egui::Slider::new(&mut self.cfg.poll_secs, 15..=600).logarithmic(true).suffix(" s"));
            ui.end_row();
            ui.label("Startup");
            let mut auto = self.autostart;
            if ui.checkbox(&mut auto, "Start with Windows").changed() && autostart::set(auto).is_ok() {
                self.autostart = auto;
            }
            ui.end_row();
        });
    }

    /// First run only: once the auto-sized table columns have settled, make the window as tall
    /// as its content (within the screen), so there's neither a scrollbar nor empty space.
    fn fit_height(&mut self, ctx: &egui::Context, content_height: f32) {
        if !self.fit {
            return;
        }
        self.frames += 1;
        if self.frames < 3 {
            ctx.request_repaint();
            return;
        }
        self.fit = false;
        let screen = ctx.input(|i| i.viewport().monitor_size).map_or(1000.0, |s| s.y);
        let width = ctx.input(|i| i.screen_rect().width());
        let height = (content_height + 40.0).clamp(420.0, screen * 0.85);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(width, height)));
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
        .map(|r| {
            let charging = if r.charging == Some(true) { " · charging" } else { "" };
            match &r.level {
                battery_core::Level::Percent(p) if r.connected == Some(false) => format!("disconnected · last seen {p}%"),
                battery_core::Level::Percent(p) => format!("{p}%{charging}"),
                battery_core::Level::Coarse(s) => s.clone(),
                // The reason already says off/asleep/no reply; only add "connected" when it is.
                battery_core::Level::Unavailable(s) if r.connected == Some(true) => format!("connected · {s}"),
                battery_core::Level::Unavailable(s) => s.clone(),
            }
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

        let content = egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(16))
            .show(ctx, |ui| {
                // The preview stays put above the tabs.
                self.preview(ui);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    for t in Tab::ALL {
                        let text = RichText::new(t.name()).size(15.0);
                        let text = if self.tab == t { text.strong() } else { text };
                        ui.selectable_value(&mut self.tab, t, text);
                    }
                });
                ui.separator();
                ui.add_space(6.0);
                let fixed = ui.cursor().top() - ui.max_rect().top();
                let scroll = egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                    match self.tab {
                        Tab::Devices => {
                            self.devices(ui);
                            ui.add_space(6.0);
                            ui.checkbox(&mut self.cfg.show_offline, "Show disconnected devices (dimmed)");
                        }
                        Tab::Display => self.display(ui),
                        Tab::Behaviour => self.behaviour(ui),
                        Tab::About => self.about(ui),
                    }
                    ui.min_rect().height()
                });
                fixed + scroll.inner
            })
            .inner;
        self.fit_height(ctx, content);

        self.apply();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(FITTED_KEY, "1".into());
        storage.set_string(TAB_KEY, self.tab.name().into());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.set_highlight(None);
    }
}
