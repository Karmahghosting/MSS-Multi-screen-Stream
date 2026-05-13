//! MSS — Multi-Screen Stream launcher GUI.
//!
//! A standalone binary that spawns the `p2p-screenshare` CLI as a child process,
//! parses its stdout for per-monitor stats, and shows them live.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, FontFamily, FontId, Rounding, Stroke, Vec2};
use egui_plot::{Line, Plot, PlotPoints};
use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};

const LOG_MAX_LINES: usize = 1500;
const STATS_HISTORY: usize = 90;

// Brand palette (dark, cyan accent)
const BG: Color32 = Color32::from_rgb(0x0a, 0x0e, 0x1a);
const PANEL: Color32 = Color32::from_rgb(0x13, 0x18, 0x25);
const CARD: Color32 = Color32::from_rgb(0x1c, 0x23, 0x33);
const CARD_HOVER: Color32 = Color32::from_rgb(0x25, 0x2d, 0x40);
const ACCENT: Color32 = Color32::from_rgb(0x06, 0xb6, 0xd4); // cyan-500
const ACCENT_DIM: Color32 = Color32::from_rgb(0x08, 0x74, 0x88);
const SUCCESS: Color32 = Color32::from_rgb(0x10, 0xb9, 0x81);
const DANGER: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
const TEXT: Color32 = Color32::from_rgb(0xe2, 0xe8, 0xf0);
const TEXT_MUTED: Color32 = Color32::from_rgb(0x94, 0xa3, 0xb8);
const BORDER: Color32 = Color32::from_rgb(0x33, 0x3d, 0x52);

// ---------- Model ----------

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Share,
    View,
}

#[derive(Clone, PartialEq, Eq)]
enum Page {
    Home,
    Configure(Mode),
    Running(Mode),
}

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    bind: String,
    connect: String,
    fps: u32,
    quality: u8,
    skip_unchanged: bool,
    #[serde(default)]
    recent_connects: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            bind: "0.0.0.0:9000".into(),
            connect: "127.0.0.1:9000".into(),
            fps: 60,
            quality: 70,
            skip_unchanged: true,
            recent_connects: Vec::new(),
        }
    }
}

impl Settings {
    fn path() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "mss", "mss-stream")
            .map(|d| d.config_dir().join("settings.json"))
    }
    fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(&p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, json);
            }
        }
    }
    fn push_recent(&mut self, c: &str) {
        let c = c.trim();
        if c.is_empty() {
            return;
        }
        self.recent_connects.retain(|r| r != c);
        self.recent_connects.insert(0, c.into());
        self.recent_connects.truncate(8);
    }
}

struct MonitorStats {
    fps: f32,
    kbps: f32,
    history_fps: VecDeque<f32>,
    history_kbps: VecDeque<f32>,
    last_update: Instant,
}

impl MonitorStats {
    fn new() -> Self {
        MonitorStats {
            fps: 0.0,
            kbps: 0.0,
            history_fps: VecDeque::with_capacity(STATS_HISTORY),
            history_kbps: VecDeque::with_capacity(STATS_HISTORY),
            last_update: Instant::now(),
        }
    }
    fn record(&mut self, fps: f32, kbps: f32) {
        self.fps = fps;
        self.kbps = kbps;
        if self.history_fps.len() == STATS_HISTORY {
            self.history_fps.pop_front();
        }
        if self.history_kbps.len() == STATS_HISTORY {
            self.history_kbps.pop_front();
        }
        self.history_fps.push_back(fps);
        self.history_kbps.push_back(kbps);
        self.last_update = Instant::now();
    }
}

struct RunningChild {
    child: Child,
    stop: Arc<AtomicBool>,
}

struct App {
    page: Page,
    settings: Settings,
    runner: Option<RunningChild>,
    log: Arc<Mutex<VecDeque<String>>>,
    stats: Arc<Mutex<HashMap<u8, MonitorStats>>>,
    n_local_monitors: usize,
    cli_path: PathBuf,
    started_at: Option<Instant>,
}

// ---------- Helpers ----------

fn locate_cli_binary() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let dir = exe.parent().unwrap_or_else(|| std::path::Path::new("."));
    let name = if cfg!(windows) {
        "p2p-screenshare.exe"
    } else {
        "p2p-screenshare"
    };
    let cand = dir.join(name);
    if cand.exists() {
        cand
    } else {
        PathBuf::from(name)
    }
}

fn detect_monitor_count() -> usize {
    scrap::Display::all().map(|v| v.len()).unwrap_or(0)
}

fn push_log(log: &Arc<Mutex<VecDeque<String>>>, line: String) {
    let mut l = log.lock();
    if l.len() == LOG_MAX_LINES {
        l.pop_front();
    }
    l.push_back(line);
}

fn pump_lines<R: Read + Send + 'static>(
    r: R,
    log: Arc<Mutex<VecDeque<String>>>,
    stats: Option<Arc<Mutex<HashMap<u8, MonitorStats>>>>,
    stop: Arc<AtomicBool>,
    is_stderr: bool,
) {
    let re = Regex::new(r"m(\d+):\s*([0-9.]+)fps\s+([0-9.]+)KB/s").unwrap();
    let r = BufReader::new(r);
    for line in r.lines() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(line) = line else { return };
        if let Some(stats) = &stats {
            let mut s = stats.lock();
            for cap in re.captures_iter(&line) {
                let id: u8 = cap[1].parse().unwrap_or(0);
                let fps: f32 = cap[2].parse().unwrap_or(0.0);
                let kbps: f32 = cap[3].parse().unwrap_or(0.0);
                s.entry(id).or_insert_with(MonitorStats::new).record(fps, kbps);
            }
        }
        let prefixed = if is_stderr {
            format!("[err] {line}")
        } else {
            line
        };
        push_log(&log, prefixed);
    }
}

impl App {
    fn new() -> Self {
        App {
            page: Page::Home,
            settings: Settings::load(),
            runner: None,
            log: Arc::new(Mutex::new(VecDeque::with_capacity(LOG_MAX_LINES))),
            stats: Arc::new(Mutex::new(HashMap::new())),
            n_local_monitors: detect_monitor_count(),
            cli_path: locate_cli_binary(),
            started_at: None,
        }
    }

    fn start_child(&mut self, mode: Mode) {
        if self.runner.is_some() {
            return;
        }
        if !self.cli_path.exists() {
            push_log(
                &self.log,
                format!("[gui] CLI binary not found at {}", self.cli_path.display()),
            );
            return;
        }

        self.stats.lock().clear();
        self.log.lock().clear();

        let mut cmd = Command::new(&self.cli_path);
        match mode {
            Mode::Share => {
                cmd.arg("share")
                    .arg("--bind")
                    .arg(&self.settings.bind)
                    .arg("--fps")
                    .arg(self.settings.fps.to_string())
                    .arg("--quality")
                    .arg(self.settings.quality.to_string())
                    .arg("--skip-unchanged")
                    .arg(self.settings.skip_unchanged.to_string());
            }
            Mode::View => {
                cmd.arg("view").arg("--connect").arg(&self.settings.connect);
                let c = self.settings.connect.clone();
                self.settings.push_recent(&c);
                self.settings.save();
            }
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        push_log(&self.log, format!("[gui] launching {mode:?}"));

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                push_log(&self.log, format!("[gui] spawn failed: {e}"));
                return;
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        if let Some(out) = child.stdout.take() {
            let log = self.log.clone();
            let stats = self.stats.clone();
            let stop_c = stop.clone();
            thread::spawn(move || pump_lines(out, log, Some(stats), stop_c, false));
        }
        if let Some(err) = child.stderr.take() {
            let log = self.log.clone();
            let stop_c = stop.clone();
            thread::spawn(move || pump_lines(err, log, None, stop_c, true));
        }

        self.runner = Some(RunningChild { child, stop });
        self.started_at = Some(Instant::now());
        self.page = Page::Running(mode);
    }

    fn stop_child(&mut self) {
        if let Some(mut r) = self.runner.take() {
            r.stop.store(true, Ordering::Relaxed);
            let _ = r.child.kill();
            let _ = r.child.wait();
            push_log(&self.log, "[gui] stopped".into());
        }
        self.started_at = None;
    }

    fn poll_child(&mut self) {
        let exited = match &mut self.runner {
            Some(r) => match r.child.try_wait() {
                Ok(Some(status)) => {
                    push_log(&self.log, format!("[gui] child exited: {status}"));
                    true
                }
                Ok(None) => false,
                Err(e) => {
                    push_log(&self.log, format!("[gui] wait error: {e}"));
                    true
                }
            },
            None => false,
        };
        if exited {
            self.runner = None;
        }
    }
}

// ---------- Theme ----------

fn apply_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.window_fill = BG;
    style.visuals.panel_fill = BG;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = CARD;
    style.visuals.code_bg_color = CARD;
    let r = Rounding::same(8.0);
    style.visuals.widgets.noninteractive.bg_fill = PANEL;
    style.visuals.widgets.noninteractive.weak_bg_fill = PANEL;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.bg_fill = CARD;
    style.visuals.widgets.inactive.weak_bg_fill = CARD;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.inactive.rounding = r;
    style.visuals.widgets.hovered.bg_fill = CARD_HOVER;
    style.visuals.widgets.hovered.weak_bg_fill = CARD_HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.hovered.rounding = r;
    style.visuals.widgets.active.bg_fill = ACCENT_DIM;
    style.visuals.widgets.active.weak_bg_fill = ACCENT_DIM;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.active.rounding = r;
    style.visuals.selection.bg_fill = ACCENT_DIM;
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.window_rounding = Rounding::same(12.0);
    style.visuals.menu_rounding = Rounding::same(8.0);
    style.visuals.hyperlink_color = ACCENT;
    style.spacing.item_spacing = Vec2::new(10.0, 10.0);
    style.spacing.button_padding = Vec2::new(14.0, 8.0);
    style.spacing.slider_width = 240.0;

    use egui::TextStyle;
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(28.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Body,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Small, FontId::new(11.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(12.0, FontFamily::Monospace),
    );

    ctx.set_style(style);
}

// ---------- Widgets ----------

fn card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .rounding(Rounding::same(12.0))
        .inner_margin(egui::Margin::same(18.0))
        .show(ui, |ui| body(ui));
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .color(TEXT_MUTED)
            .size(11.0)
            .strong(),
    );
    ui.add_space(4.0);
}

fn big_button(ui: &mut egui::Ui, text: &str, accent: bool) -> egui::Response {
    let color = if accent { Color32::WHITE } else { TEXT };
    let txt = egui::RichText::new(text).size(15.0).strong().color(color);
    let fill = if accent { ACCENT } else { CARD_HOVER };
    let stroke = if accent { ACCENT } else { BORDER };
    let btn = egui::Button::new(txt)
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .rounding(Rounding::same(10.0))
        .min_size(Vec2::new(0.0, 42.0));
    ui.add(btn)
}

fn pulsing_dot(ui: &mut egui::Ui, color: Color32, time: f64) {
    let alpha = (((time * 2.5).sin() * 0.4 + 0.6).clamp(0.2, 1.0)) as f32;
    let c = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (255.0 * alpha) as u8);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(14.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.5, c);
    ui.painter()
        .circle_stroke(rect.center(), 5.5, Stroke::new(1.0, color));
}

// ---------- Pages ----------

impl App {
    fn ui_top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("◆ MSS")
                    .strong()
                    .size(18.0)
                    .color(ACCENT),
            );
            ui.label(
                egui::RichText::new(" / Multi-Screen Stream")
                    .color(TEXT_MUTED)
                    .size(13.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                        .color(TEXT_MUTED)
                        .small(),
                );
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{} display(s)", self.n_local_monitors))
                        .color(TEXT_MUTED)
                        .small(),
                );
            });
        });
        ui.add_space(6.0);
        ui.painter().hline(
            ui.cursor().x_range(),
            ui.cursor().min.y,
            Stroke::new(1.0, BORDER),
        );
        ui.add_space(14.0);
    }

    fn ui_home(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("Stream screens, peer-to-peer.")
                    .size(24.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new(
                    "Every remote monitor lands on one of your local monitors, fullscreen.",
                )
                .color(TEXT_MUTED)
                .size(13.0),
            );
        });
        ui.add_space(28.0);

        let mut go_share = false;
        let mut go_view = false;

        ui.horizontal(|ui| {
            let avail = ui.available_width();
            let card_w = (avail - 16.0) / 2.0;

            ui.allocate_ui(Vec2::new(card_w, 240.0), |ui| {
                card(ui, |ui| {
                    ui.set_min_height(200.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("📡").size(32.0));
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Share")
                                    .size(22.0)
                                    .strong()
                                    .color(ACCENT),
                            );
                            ui.label(
                                egui::RichText::new("Sender")
                                    .color(TEXT_MUTED)
                                    .size(11.0),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "Capture every local screen and stream them to a connecting peer. \
                             Each monitor becomes its own JPEG track.",
                        )
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(12.0);
                    if big_button(ui, "Configure  →", true).clicked() {
                        go_share = true;
                    }
                });
            });

            ui.allocate_ui(Vec2::new(card_w, 240.0), |ui| {
                card(ui, |ui| {
                    ui.set_min_height(200.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("👁").size(32.0));
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("View")
                                    .size(22.0)
                                    .strong()
                                    .color(ACCENT),
                            );
                            ui.label(
                                egui::RichText::new("Receiver")
                                    .color(TEXT_MUTED)
                                    .size(11.0),
                            );
                        });
                    });
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "Connect to a sharer. Every remote monitor opens fullscreen on one of \
                             your local monitors. Esc to quit.",
                        )
                        .color(TEXT_MUTED),
                    );
                    ui.add_space(12.0);
                    if big_button(ui, "Configure  →", false).clicked() {
                        go_view = true;
                    }
                });
            });
        });

        if go_share {
            self.page = Page::Configure(Mode::Share);
        }
        if go_view {
            self.page = Page::Configure(Mode::View);
        }

        ui.add_space(24.0);

        if !self.settings.recent_connects.is_empty() {
            section_label(ui, "RECENT");
            card(ui, |ui| {
                let recents = self.settings.recent_connects.clone();
                for c in recents {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("•").color(ACCENT));
                        if ui
                            .link(egui::RichText::new(&c).monospace())
                            .clicked()
                        {
                            self.settings.connect = c.clone();
                            self.page = Page::Configure(Mode::View);
                        }
                    });
                }
            });
        }
    }

    fn ui_configure(&mut self, ui: &mut egui::Ui, mode: Mode) {
        ui.horizontal(|ui| {
            if ui.button("←  Home").clicked() {
                self.page = Page::Home;
            }
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(match mode {
                    Mode::Share => "Share — configure",
                    Mode::View => "View — configure",
                })
                .size(20.0)
                .strong(),
            );
        });
        ui.add_space(18.0);

        match mode {
            Mode::Share => self.ui_configure_share(ui),
            Mode::View => self.ui_configure_view(ui),
        }
    }

    fn ui_configure_share(&mut self, ui: &mut egui::Ui) {
        section_label(ui, "NETWORK");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Bind address");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.bind)
                        .desired_width(240.0)
                        .hint_text("host:port"),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Peers connect to this address. Use 0.0.0.0 to accept any interface.",
                )
                .color(TEXT_MUTED)
                .small(),
            );
        });
        ui.add_space(12.0);

        section_label(ui, "ENCODING");
        card(ui, |ui| {
            egui::Grid::new("enc-grid")
                .num_columns(2)
                .spacing([16.0, 12.0])
                .show(ui, |ui| {
                    ui.label("Target FPS");
                    ui.add(egui::Slider::new(&mut self.settings.fps, 1..=120));
                    ui.end_row();

                    ui.label("JPEG quality");
                    ui.add(egui::Slider::new(&mut self.settings.quality, 1..=100));
                    ui.end_row();

                    ui.label("Skip unchanged");
                    ui.checkbox(
                        &mut self.settings.skip_unchanged,
                        "xxh3 frame hash · idle ≈ 0 CPU",
                    );
                    ui.end_row();
                });
        });
        ui.add_space(12.0);

        section_label(ui, "DISPLAYS");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}", self.n_local_monitors))
                        .size(40.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new("monitors detected").color(TEXT).size(14.0),
                    );
                    ui.label(
                        egui::RichText::new(
                            "Each will be captured and sent as its own JPEG stream.",
                        )
                        .color(TEXT_MUTED)
                        .small(),
                    );
                });
            });
        });
        ui.add_space(20.0);

        if big_button(ui, "Start streaming  →", true).clicked() {
            self.settings.save();
            self.start_child(Mode::Share);
        }
    }

    fn ui_configure_view(&mut self, ui: &mut egui::Ui) {
        section_label(ui, "CONNECTION");
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Connect to");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings.connect)
                        .desired_width(260.0)
                        .hint_text("host:port"),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Press Esc inside any viewer window to quit. Each remote monitor opens \
                     fullscreen on a different local one.",
                )
                .color(TEXT_MUTED)
                .small(),
            );
        });
        ui.add_space(12.0);

        if !self.settings.recent_connects.is_empty() {
            section_label(ui, "RECENT");
            card(ui, |ui| {
                let recents = self.settings.recent_connects.clone();
                for c in recents {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("•").color(ACCENT));
                        if ui.link(egui::RichText::new(&c).monospace()).clicked() {
                            self.settings.connect = c.clone();
                        }
                    });
                }
            });
            ui.add_space(12.0);
        }

        if big_button(ui, "Connect  →", true).clicked() {
            self.settings.save();
            self.start_child(Mode::View);
        }
    }

    fn ui_running(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, mode: Mode) {
        let live = self.runner.is_some();
        let time = ctx.input(|i| i.time);

        // Status bar
        ui.horizontal(|ui| {
            pulsing_dot(ui, if live { SUCCESS } else { DANGER }, time);
            ui.label(
                egui::RichText::new(if live { "LIVE" } else { "STOPPED" })
                    .strong()
                    .size(13.0)
                    .color(if live { SUCCESS } else { DANGER }),
            );
            ui.separator();
            let summary = match mode {
                Mode::Share => format!("Share · {}", self.settings.bind),
                Mode::View => format!("View · {}", self.settings.connect),
            };
            ui.label(
                egui::RichText::new(summary)
                    .color(TEXT_MUTED)
                    .monospace(),
            );
            if let Some(start) = self.started_at {
                let secs = start.elapsed().as_secs();
                let m = secs / 60;
                let s = secs % 60;
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("⏱ {m:02}:{s:02}"))
                        .color(TEXT_MUTED)
                        .monospace(),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if live {
                    let btn = egui::Button::new(
                        egui::RichText::new("◼  Stop").strong().color(Color32::WHITE),
                    )
                    .fill(DANGER)
                    .rounding(Rounding::same(8.0))
                    .min_size(Vec2::new(0.0, 32.0));
                    if ui.add(btn).clicked() {
                        self.stop_child();
                    }
                } else if ui
                    .add(
                        egui::Button::new("←  Home")
                            .rounding(Rounding::same(8.0))
                            .min_size(Vec2::new(0.0, 32.0)),
                    )
                    .clicked()
                {
                    self.page = Page::Home;
                }
            });
        });
        ui.add_space(8.0);

        // Per-monitor stats
        section_label(ui, "PER-MONITOR PERFORMANCE");
        card(ui, |ui| {
            let stats = self.stats.lock();
            if stats.is_empty() {
                ui.label(
                    egui::RichText::new("⌛  Waiting for first frame…")
                        .color(TEXT_MUTED),
                );
            } else {
                let mut ids: Vec<u8> = stats.keys().copied().collect();
                ids.sort();
                for id in ids {
                    let m = &stats[&id];
                    let fresh = m.last_update.elapsed() < Duration::from_secs(3);
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("m{id}"))
                                .strong()
                                .monospace()
                                .color(if fresh { ACCENT } else { TEXT_MUTED }),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:>6.1} fps", m.fps))
                                .monospace()
                                .size(14.0),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:>8.1} KB/s", m.kbps))
                                .monospace()
                                .color(TEXT_MUTED),
                        );

                        let pts: PlotPoints = m
                            .history_fps
                            .iter()
                            .enumerate()
                            .map(|(i, v)| [i as f64, *v as f64])
                            .collect();
                        let line = Line::new(pts).color(ACCENT).width(1.5);
                        Plot::new(format!("plot-fps-{id}"))
                            .height(36.0)
                            .show_x(false)
                            .show_y(false)
                            .show_axes([false, false])
                            .show_grid([false, false])
                            .allow_scroll(false)
                            .allow_drag(false)
                            .allow_zoom(false)
                            .include_y(0.0)
                            .show(ui, |pui| pui.line(line));
                    });
                    ui.add_space(2.0);
                }
            }
        });
        ui.add_space(12.0);

        // Log
        section_label(ui, "LOG");
        let log_height = ui.available_height().max(140.0);
        egui::Frame::none()
            .fill(BG)
            .stroke(Stroke::new(1.0, BORDER))
            .rounding(Rounding::same(8.0))
            .inner_margin(egui::Margin::same(10.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(log_height - 30.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let lines: Vec<String> = self.log.lock().iter().cloned().collect();
                        for line in lines {
                            let color = if line.starts_with("[err]") {
                                DANGER
                            } else if line.starts_with("[gui]") {
                                ACCENT
                            } else if line.starts_with("[share]") || line.starts_with("[view") {
                                TEXT
                            } else {
                                TEXT_MUTED
                            };
                            ui.label(
                                egui::RichText::new(line)
                                    .color(color)
                                    .monospace()
                                    .size(11.5),
                            );
                        }
                    });
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_child();

        if self.runner.is_some() {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .fill(BG)
                    .inner_margin(egui::Margin::same(24.0)),
            )
            .show(ctx, |ui| {
                self.ui_top_bar(ui);
                let page = self.page.clone();
                match page {
                    Page::Home => self.ui_home(ui),
                    Page::Configure(mode) => self.ui_configure(ui, mode),
                    Page::Running(mode) => self.ui_running(ui, ctx, mode),
                }
            });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.stop_child();
        self.settings.save();
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 660.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("MSS — Multi-Screen Stream"),
        ..Default::default()
    };
    eframe::run_native(
        "MSS",
        options,
        Box::new(|cc| {
            apply_theme(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
