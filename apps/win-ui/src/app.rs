//! egui application: state, host event handling and screens.

use crate::config::*;
use crate::control::*;
use crate::driver::{self, DriverResult};
use crate::host::{host_command, new_channel, HostProcess, UiMsg};
use crate::util;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, StrokeKind, Vec2};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

const RELEASE_URL: &str = "https://github.com/XclipseX/window-remote-view/releases/tag/latest-build";
const MAX_LOGS: usize = 2000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Settings,
    Guide,
    Logs,
}

/// What the big status card shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Idle,
    Starting,
    Waiting,
    Connecting,
    Streaming,
    Error,
}

struct Demo {
    started: Instant,
    last_stats: Instant,
    tick: u32,
}

pub struct App {
    cfg: Config,
    page: Page,
    rx: Receiver<UiMsg>,
    tx: std::sync::mpsc::Sender<UiMsg>,
    host: Option<HostProcess>,
    running: bool,
    status: Option<StatusEvent>,
    error: Option<String>,
    stats: StatsEvent,
    fps_hist: VecDeque<(Instant, f32)>,
    devices: Vec<DeviceItem>,
    logs: VecDeque<(Duration, LogLevel, String)>,
    t0: Instant,
    pin: Option<String>,
    /// GUI-side device scanner runs only while no host engine is running.
    scan_on: Arc<AtomicBool>,
    /// Held by the scanner while it touches USB; `start()` waits on it so the
    /// GUI never has a device open when the host worker begins.
    scan_lock: Arc<std::sync::Mutex<()>>,
    /// (vid, pid) of the driver install in progress.
    driver_busy: Option<(u16, u16)>,
    driver_tried: std::collections::HashSet<(u16, u16)>,
    driver_note: Option<(bool, String)>,
    /// Virtual display driver install in progress / already attempted this run.
    vdd_busy: bool,
    vdd_tried: bool,
    vdd_present: bool,
    /// RemoteView service (shows / controls the lock screen) is registered.
    svc_present: bool,
    /// Installed service points to a different exe or is still running the pre-update image.
    svc_needs_refresh: bool,
    /// Connect to this device as soon as its driver install succeeds.
    connect_after_driver: Option<DeviceItem>,
    local_ip: String,
    demo: Option<Demo>,
    autostart_msg: Option<String>,
    ctx: egui::Context,
    really_quit: bool,
    tray: Option<crate::tray::TrayHandle>,
    tray_tip: String,
    upd_tx: std::sync::mpsc::Sender<crate::update::UpdateMsg>,
    upd_rx: Receiver<crate::update::UpdateMsg>,
    upd: UpdateState,
    /// Outputs shown in the "Màn hình cần chia sẻ" picker.
    outputs: Vec<rv_host_win::capture::OutputInfo>,
    /// Result of the last "Xuất log" (success?, text).
    export_note: Option<(bool, String)>,
    /// Delayed ZIP export timer after asking Android for its log over the active session.
    pending_log_export: Option<Instant>,
    /// Last time a concise `stats:` summary line was appended to the log file.
    last_stats_log: Option<Instant>,
    /// Coalesces rapid quality-slider drags into one `SetQuality` command.
    quality_debounce: QualityDebounce,
    /// Start sharing automatically once the driver/service auto-install is out of the way.
    pending_auto_share: bool,
    /// Launched with `--minimized` (autostart): minimize the window on the first frame.
    minimize_first_frame: bool,
}

/// Coalesces rapid quality-slider drags into a single `SetQuality` command
/// after `DEBOUNCE` (or immediately when the user releases the drag).
#[derive(Debug, Default)]
struct QualityDebounce {
    pending: Option<(Instant, u8)>,
}

impl QualityDebounce {
    const DEBOUNCE: Duration = Duration::from_millis(150);

    fn schedule(&mut self, now: Instant, quality: u8) -> Duration {
        self.pending = Some((now + Self::DEBOUNCE, quality));
        Self::DEBOUNCE
    }

    fn clear(&mut self) {
        self.pending = None;
    }

    fn flush_now(&mut self) -> Option<u8> {
        self.pending.take().map(|(_, q)| q)
    }

    /// Returns `(Some(quality), None)` when due, or `(None, Some(remaining))` while waiting.
    fn poll(&mut self, now: Instant) -> (Option<u8>, Option<Duration>) {
        let Some((due, q)) = self.pending else {
            return (None, None);
        };
        if now >= due {
            self.pending = None;
            (Some(q), None)
        } else {
            (None, Some(due - now))
        }
    }
}

#[derive(Default)]
enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(crate::update::UpdateInfo),
    Downloading(u64, u64),
    Inaccessible,
    Error(String),
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let dark = ui.visuals().dark_mode;
    let fill = if dark { Color32::from_rgb(36, 38, 47) } else { Color32::WHITE };
    let stroke = if dark { Color32::from_rgb(52, 55, 66) } else { Color32::from_rgb(226, 229, 238) };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0_f32, stroke))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

/// FPS the host can actually reach: the software encoder is capped at 60 by the engine.
fn effective_target_fps(cfg_fps: u32, encoder: &str) -> f32 {
    let cap = if encoder.to_ascii_lowercase().contains("openh264") { 60 } else { u32::MAX };
    cfg_fps.min(cap).max(1) as f32
}

/// 1% low colour: green >= 90 % of the target fps, amber >= 60 %, red below.
fn low1_color(low1: f32, target: f32) -> Color32 {
    match low1 / target.max(1.0) {
        r if r >= 0.9 => Color32::from_rgb(52, 190, 100),
        r if r >= 0.6 => Color32::from_rgb(235, 170, 40),
        _ => Color32::from_rgb(225, 70, 70),
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(20.0).strong());
}

fn accent() -> Color32 {
    Color32::from_rgb(52, 120, 246)
}

impl App {
    /// Font mặc định của egui thiếu dấu tiếng Việt: nạp font hệ thống có đủ glyph.
    fn setup_fonts(ctx: &egui::Context) {
        const CANDIDATES: &[&str] = &[
            "C:\\Windows\\Fonts\\segoeui.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
            "C:\\Windows\\Fonts\\tahoma.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ];
        let Some(bytes) = CANDIDATES.iter().find_map(|p| std::fs::read(p).ok()) else {
            return;
        };
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "system-vi".to_owned(),
            std::sync::Arc::new(egui::FontData::from_owned(bytes)),
        );
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().insert(0, "system-vi".to_owned());
        }
        ctx.set_fonts(fonts);
    }

    pub fn new(cc: &eframe::CreationContext<'_>, demo: bool, minimized: bool, just_updated: bool) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self::setup_style(&cc.egui_ctx);
        let (tx, rx) = new_channel();
        let (upd_tx, upd_rx) = std::sync::mpsc::channel();
        let cfg = Config::load();
        let pending_auto_share = !demo && (cfg.auto_share || minimized);
        let svc_present = driver::service_installed();
        let svc_repoint = crate::service::service_needs_repoint();
        let svc_needs_refresh = cfg!(windows) && svc_present && (svc_repoint || just_updated);
        let tray = crate::tray::TrayHandle::spawn(cc.egui_ctx.clone());
        let mut app = Self {
            cfg,
            page: Page::Home,
            rx,
            tx,
            host: None,
            running: false,
            status: None,
            error: None,
            stats: StatsEvent::default(),
            fps_hist: VecDeque::new(),
            devices: Vec::new(),
            logs: VecDeque::new(),
            t0: Instant::now(),
            pin: None,
            scan_on: Arc::new(AtomicBool::new(!demo)),
            scan_lock: Arc::new(std::sync::Mutex::new(())),
            driver_busy: None,
            driver_tried: Default::default(),
            driver_note: None,
            vdd_busy: false,
            vdd_tried: false,
            vdd_present: rv_host_win::sudovda::driver_present(),
            svc_present,
            svc_needs_refresh,
            connect_after_driver: None,
            local_ip: util::local_ip(),
            demo: None,
            autostart_msg: None,
            ctx: cc.egui_ctx.clone(),
            really_quit: false,
            tray,
            tray_tip: String::new(),
            upd_tx,
            upd_rx,
            upd: UpdateState::Idle,
            outputs: Vec::new(),
            export_note: None,
            pending_log_export: None,
            last_stats_log: None,
            quality_debounce: QualityDebounce::default(),
            pending_auto_share,
            minimize_first_frame: minimized && !demo,
        };
        if !demo {
            app.refresh_outputs();
            if app.svc_needs_refresh {
                let msg = if svc_repoint {
                    "Dịch vụ RemoteView đang trỏ sang đường dẫn khác; nhấn \"Cập nhật dịch vụ\" để đồng bộ."
                } else {
                    "Ứng dụng vừa cập nhật; nhấn \"Cập nhật dịch vụ\" để khởi động lại dịch vụ RemoteView với bản mới."
                };
                app.log(LogLevel::Warn, msg.into());
            }
            // Cài driver màn hình ảo + dịch vụ RemoteView (xem/điều khiển cả màn hình khóa) ngay lần
            // đầu mở app: một hộp thoại UAC, không cần bấm nút. Thiếu dịch vụ (driver đã có) vẫn chạy một lần.
            if cfg!(windows) && !app.cfg.vdd_auto_declined {
                if driver::vdd_bundled() && !app.vdd_present {
                    app.install_vdd(false);
                } else if !app.svc_present {
                    app.install_service();
                }
            }
        }
        if !demo && app.cfg.auto_update_check {
            app.check_update(false);
        }
        if !demo {
            app.spawn_scanner();
        }
        if demo {
            app.cfg = Config::default();
            app.start();
            app.demo.as_mut().unwrap().started -= Duration::from_secs(5);
        }
        app.update_tray_tooltip();
        app
    }

    /// Lists attached devices every 2 s while no host engine is running, so the
    /// home screen shows device cards before the user presses anything.
    fn spawn_scanner(&self) {
        let (tx, on, ctx, lock) = (self.tx.clone(), self.scan_on.clone(), self.ctx.clone(), self.scan_lock.clone());
        std::thread::spawn(move || {
            let (mut last, mut last_name): (Option<Vec<DeviceItem>>, Option<String>) = (None, None);
            loop {
                if on.load(Ordering::Relaxed) {
                    let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    if !on.load(Ordering::Relaxed) {
                        drop(guard);
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    let items: Vec<DeviceItem> =
                        rv_host_win::engine::scan_devices(&mut last_name).into_iter().map(Into::into).collect();
                    drop(guard);
                    if on.load(Ordering::Relaxed) && last.as_ref() != Some(&items) {
                        last = Some(items.clone());
                        if tx.send(UiMsg::Scan(items)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                } else {
                    last = None;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    fn setup_style(ctx: &egui::Context) {
        ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::System);
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| {
                s.spacing.item_spacing = Vec2::new(10.0, 10.0);
                s.spacing.button_padding = Vec2::new(14.0, 8.0);
                s.spacing.interact_size.y = 30.0;
                let r = CornerRadius::same(10);
                s.visuals.widgets.noninteractive.corner_radius = r;
                s.visuals.widgets.inactive.corner_radius = r;
                s.visuals.widgets.hovered.corner_radius = r;
                s.visuals.widgets.active.corner_radius = r;
                s.visuals.widgets.open.corner_radius = r;
                s.visuals.selection.bg_fill = accent();
                s.visuals.panel_fill = if theme == egui::Theme::Dark {
                    Color32::from_rgb(24, 25, 31)
                } else {
                    Color32::from_rgb(243, 245, 250)
                };
                for ts in s.text_styles.values_mut() {
                    ts.size *= 1.12;
                }
            });
        }
    }

    // ---- host control ----

    /// True when the next session needs RemoteView's own virtual monitor: asked for, or the
    /// laptop has no active display (broken panel and no TeamViewer/spacedesk monitor).
    fn needs_vdd(&self) -> bool {
        self.cfg.display != Display::Mirror || self.outputs.is_empty()
    }

    fn install_vdd(&mut self, then_start: bool) {
        self.vdd_busy = true;
        self.vdd_tried = true;
        self.driver_note = Some((true, "Đang cài driver màn hình ảo (SudoVDA)... hãy bấm \"Có\" ở hộp thoại quyền quản trị.".into()));
        self.log(LogLevel::Info, "Đang cài driver màn hình ảo SudoVDA".into());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = driver::install_vdd();
            let _ = tx.send(UiMsg::Vdd((r, then_start)));
            ctx.request_repaint();
        });
    }

    /// Chỉ cài dịch vụ RemoteView (driver màn hình ảo đã có): một hộp thoại UAC.
    fn install_service(&mut self) {
        self.vdd_busy = true;
        self.driver_note = Some((true, "Đang cài dịch vụ RemoteView (xem được cả màn hình khóa)... hãy bấm \"Có\" ở hộp thoại quyền quản trị.".into()));
        self.log(LogLevel::Info, "Đang cài dịch vụ RemoteView".into());
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = driver::install_service();
            let _ = tx.send(UiMsg::Vdd((r, false)));
            ctx.request_repaint();
        });
    }

    fn vdd_done(&mut self, (result, then_start): (DriverResult, bool)) {
        self.vdd_busy = false;
        self.vdd_present = rv_host_win::sudovda::driver_present();
        self.svc_present = driver::service_installed();
        match result {
            DriverResult::Installed => {
                self.svc_needs_refresh = crate::service::service_needs_repoint();
                let _ = crate::update::cleanup_old();
                self.log(LogLevel::Info, "Đã cài driver màn hình ảo và dịch vụ RemoteView".into());
                self.driver_note = Some((true, "Đã cài driver màn hình ảo và dịch vụ RemoteView (xem được cả màn hình khóa).".into()));
            }
            DriverResult::Cancelled => {
                // Không hỏi lại mỗi lần mở app; vẫn còn nút "Cài / cài lại" và lần chia sẻ cần màn ảo.
                self.cfg.vdd_auto_declined = true;
                self.cfg.save();
                self.log(LogLevel::Warn, "Bạn đã từ chối quyền quản trị, chưa cài driver màn hình ảo".into());
                self.driver_note = Some((false, "Chưa cài driver màn hình ảo / dịch vụ vì bạn từ chối quyền quản trị.".into()));
            }
            DriverResult::Failed(e) => {
                self.log(LogLevel::Error, format!("Cài driver màn hình ảo thất bại: {e}"));
                self.driver_note = Some((false, format!("Cài driver màn hình ảo thất bại: {e}")));
            }
        }
        if then_start && !self.running {
            self.start();
        }
    }

    /// Zero-click start: runs once, when nothing else (driver / service install) is in progress.
    fn try_auto_share(&mut self) {
        if !self.pending_auto_share || self.running || self.vdd_busy || self.demo.is_some() {
            return;
        }
        // The user already said "no" to the UAC prompt: do not pop it up again on every launch.
        let would_prompt = self.needs_vdd() && driver::vdd_bundled() && !rv_host_win::sudovda::driver_present();
        self.pending_auto_share = false;
        if cfg!(windows) && self.cfg.vdd_auto_declined && would_prompt {
            return;
        }
        self.log(LogLevel::Info, "Tự động bắt đầu chia sẻ khi mở ứng dụng".into());
        self.start();
    }

    fn start(&mut self) {
        self.pending_auto_share = false;
        if cfg!(windows)
            && self.demo.is_none()
            && !self.vdd_busy
            && !self.vdd_tried
            && self.needs_vdd()
            && driver::vdd_bundled()
            && !rv_host_win::sudovda::driver_present()
        {
            self.install_vdd(true);
            return;
        }
        self.running = true;
        self.status = None;
        self.error = None;
        self.stats = StatsEvent::default();
        self.fps_hist.clear();
        self.last_stats_log = None;
        self.log(LogLevel::Info, format!("Bắt đầu chia sẻ ({})", self.cfg.mode.name()));
        if self.demo.is_some() || std::env::args().any(|a| a == "--demo") {
            self.demo = Some(Demo { started: Instant::now(), last_stats: Instant::now(), tick: 0 });
            return;
        }
        let (exe, mut args) = host_command(self.cfg.host_path.as_deref());
        args.extend(self.cfg.host_args());
        self.scan_on.store(false, Ordering::Relaxed);
        // Wait for an in-flight GUI scan to release its USB handles.
        drop(self.scan_lock.lock());
        let ctx = self.ctx.clone();
        self.host = Some(HostProcess::start(exe, args, self.tx.clone(), move || {
            ctx.request_repaint()
        }));
    }

    fn stop(&mut self) {
        self.log(LogLevel::Info, "Dừng chia sẻ".into());
        if let Some(h) = self.host.take() {
            h.stop();
            // Detach: dropping would block; supervisor ends itself and kills after the grace period.
            std::thread::spawn(move || drop(h));
        }
        self.demo = None;
        self.running = false;
        self.scan_on.store(true, Ordering::Relaxed);
        self.status = None;
        self.pin = None;
        self.devices.clear();
        self.fps_hist.clear();
        self.last_stats_log = None;
    }

    fn send(&self, cmd: HostCmd) {
        if let Some(h) = &self.host {
            h.send(&cmd);
        }
    }

    fn refresh_outputs(&mut self) {
        self.outputs = rv_host_win::capture::list_outputs().unwrap_or_default().into_iter().filter(|o| o.attached).collect();
    }

    /// Requests a fresh log upload from the connected Android client (when sharing)
    /// before zipping all logs onto Desktop ~900 ms later.
    fn request_export_logs(&mut self) {
        if self.host.is_some() {
            self.send(HostCmd::RequestLog);
            self.pending_log_export = Some(Instant::now() + Duration::from_millis(900));
            self.export_note = Some((true, "Đang thu thập log từ thiết bị nhận...".into()));
            self.ctx.request_repaint_after(Duration::from_millis(950));
        } else {
            self.export_logs();
        }
    }

    fn export_logs(&mut self) {
        match crate::logexport::export_logs() {
            Ok(p) => {
                self.log(LogLevel::Info, format!("Đã xuất log: {}", p.display()));
                self.export_note = Some((true, format!("Đã lưu {}", p.display())));
                crate::logexport::reveal(&p);
            }
            Err(e) => {
                self.log(LogLevel::Error, format!("Xuất log thất bại: {e}"));
                self.export_note = Some((false, e));
            }
        }
    }

    fn log(&mut self, level: LogLevel, msg: String) {
        rv_host_win::logfile::write_line(
            match level {
                LogLevel::Info => "info",
                LogLevel::Warn => "warn",
                LogLevel::Error => "error",
            },
            &msg,
        );
        if self.logs.len() >= MAX_LOGS {
            self.logs.pop_front();
        }
        self.logs.push_back((self.t0.elapsed(), level, msg));
    }

    fn handle_event(&mut self, ev: HostEvent) {
        match ev {
            HostEvent::Status(s) => {
                if let Some(otp) = &s.otp {
                    self.pin = Some(otp.clone());
                }
                if let Some(m) = &s.message {
                    self.log(LogLevel::Info, m.clone());
                }
                if s.state == State::Error {
                    self.error = s.message.clone();
                } else {
                    self.error = None;
                }
                self.status = Some(s);
            }
            HostEvent::Stats(s) => {
                let now = Instant::now();
                self.fps_hist.push_back((now, s.fps));
                while self.fps_hist.front().is_some_and(|(t, _)| now.duration_since(*t) > Duration::from_secs(60)) {
                    self.fps_hist.pop_front();
                }
                let due = self.last_stats_log.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(5));
                if due && (s.fps > 0.0 || s.width > 0) {
                    self.last_stats_log = Some(now);
                    let dec = s.decode_ms.map(|d| format!("{d:.1}ms")).unwrap_or_else(|| "-".into());
                    let rtt = s.rtt_ms.map(|r| format!("{r:.1}ms")).unwrap_or_else(|| "-".into());
                    rv_host_win::logfile::write_line(
                        "info",
                        &format!(
                            "stats: fps={:.1} avg={:.1} low1={:.1} enc={:.1}ms dec={dec} rtt={rtt} kbps={} {}x{} {}",
                            s.fps, s.fps_avg, s.fps_low1, s.encode_ms, s.bitrate_kbps, s.width, s.height, s.encoder
                        ),
                    );
                }
                self.stats = s;
            }
            HostEvent::Devices { items } => self.set_devices(items),
            HostEvent::Log { level, msg } => self.log(level, msg),
        }
    }

    fn check_update(&mut self, manual: bool) {
        self.upd = UpdateState::Checking;
        crate::update::spawn_check(self.upd_tx.clone(), manual, self.ctx.clone());
    }

    fn poll_update(&mut self) {
        use crate::update::{CheckResult, UpdateMsg};
        while let Ok(m) = self.upd_rx.try_recv() {
            match m {
                UpdateMsg::Checked { manual, result } => {
                    self.upd = match result {
                        CheckResult::Available(i) => UpdateState::Available(i),
                        CheckResult::UpToDate => UpdateState::UpToDate,
                        CheckResult::Inaccessible => UpdateState::Inaccessible,
                        // Silent on startup when offline; shown only for manual checks.
                        CheckResult::Failed(e) if manual => UpdateState::Error(e),
                        CheckResult::Failed(_) => UpdateState::Idle,
                    };
                }
                UpdateMsg::Progress { done, total } => self.upd = UpdateState::Downloading(done, total),
                UpdateMsg::Failed(e) => self.upd = UpdateState::Error(e),
                UpdateMsg::Downloaded(path) => {
                    // Stop the worker first so no child process holds locks on the exe before swap.
                    self.host = None;
                    self.running = false;
                    match crate::update::replace_self(&path) {
                        Ok(exe) => {
                            self.tray = None;
                            match crate::update::relaunch(&exe) {
                                Ok(()) => std::process::exit(0),
                                Err(e) => self.upd = UpdateState::Error(format!("không mở lại được: {e}")),
                            }
                        }
                        Err(e) => self.upd = UpdateState::Error(e),
                    }
                }
            }
        }
    }

    /// Banner + status line for updates (home banner when `banner`, else About card).
    fn update_ui(&mut self, ui: &mut egui::Ui, banner: bool) {
        let mut act = None;
        match &self.upd {
            UpdateState::Available(i) => {
                ui.horizontal(|ui| {
                    ui.colored_label(accent(), RichText::new(format!("Có bản cập nhật ({})", &i.sha[..i.sha.len().min(7)])).strong());
                    if ui.button("Tải và cập nhật").clicked() {
                        act = Some(i.clone());
                    }
                });
            }
            UpdateState::Downloading(d, t) => {
                let f = if *t > 0 { *d as f32 / *t as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(f).show_percentage().text(format!("Đang tải {:.1} / {:.1} MB", *d as f64 / 1e6, *t as f64 / 1e6)));
            }
            UpdateState::Inaccessible => {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(230, 160, 40), "Không truy cập được trang phát hành");
                    if ui.button("Mở trang tải").clicked() {
                        util::open_url(crate::update::RELEASE_URL);
                    }
                });
            }
            UpdateState::Error(e) if !banner => {
                ui.colored_label(Color32::from_rgb(230, 70, 70), format!("Lỗi cập nhật: {e}"));
            }
            UpdateState::Checking if !banner => {
                ui.weak("Đang kiểm tra...");
            }
            UpdateState::UpToDate if !banner => {
                ui.weak("Bạn đang dùng bản mới nhất.");
            }
            _ => {}
        }
        if let Some(info) = act {
            self.upd = UpdateState::Downloading(0, info.size);
            crate::update::spawn_download(info, self.upd_tx.clone(), self.ctx.clone());
        }
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        let mut actions = Vec::new();
        if let Some(tray) = &self.tray {
            while let Some(act) = tray.try_recv() {
                actions.push(act);
            }
        }
        for act in actions {
            match act {
                crate::tray::TrayAction::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                crate::tray::TrayAction::ToggleShare => {
                    if self.running {
                        self.stop();
                    } else {
                        self.start();
                    }
                }
                crate::tray::TrayAction::Quit => {
                    self.really_quit = true;
                    self.host = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn update_tray_tooltip(&mut self) {
        let Some(tray) = &self.tray else { return };
        let tip = match self.view() {
            View::Streaming => match self.status.as_ref().and_then(|s| s.peer.as_deref()) {
                Some(peer) => format!("RemoteView — Đang truyền ({peer})"),
                None => "RemoteView — Đang truyền...".to_string(),
            },
            View::Waiting | View::Starting | View::Connecting => "RemoteView — Đang chờ".to_string(),
            View::Error => "RemoteView — Lỗi".to_string(),
            View::Idle => "RemoteView — Đang chờ".to_string(),
        };
        if tip != self.tray_tip {
            tray.set_tooltip(&tip);
            self.tray_tip = tip;
        }
    }

    fn poll(&mut self) {
        self.poll_update();
        if let Some(due) = self.pending_log_export {
            let now = Instant::now();
            if now >= due {
                self.pending_log_export = None;
                self.export_logs();
            } else {
                self.ctx.request_repaint_after(due - now);
            }
        }
        let (due_q, wait_q) = self.quality_debounce.poll(Instant::now());
        if let Some(q) = due_q {
            self.cfg.save();
            self.send(HostCmd::SetQuality { quality: q });
        } else if let Some(wait) = wait_q {
            self.ctx.request_repaint_after(wait);
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::Event(e) => self.handle_event(e),
                UiMsg::Exited { code, restart_in } => {
                    let msg = format!(
                        "Tiến trình host dừng (mã {}){}",
                        code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                        restart_in.map(|d| format!(", tự khởi động lại sau {}s", d.as_secs())).unwrap_or_default()
                    );
                    self.log(LogLevel::Error, msg.clone());
                    self.error = Some(msg);
                    self.status = Some(StatusEvent {
                        state: State::Error,
                        link: Link::None,
                        peer: None,
                        otp: None,
                        message: self.error.clone(),
                    });
                    self.stats = StatsEvent::default();
                }
                UiMsg::SpawnFailed(e) => {
                    let msg = format!("Không chạy được tiến trình host: {e}");
                    self.log(LogLevel::Error, msg.clone());
                    self.host = None;
                    self.running = false;
                    self.error = Some(msg);
                }
                UiMsg::Stopped => {}
                UiMsg::Scan(items) => {
                    if !self.running {
                        self.set_devices(items);
                    }
                }
                UiMsg::Driver { vid, pid, result } => self.driver_done(vid, pid, result),
                UiMsg::Vdd(result) => self.vdd_done(result),
            }
        }
        self.tick_demo();
        self.try_auto_share();
    }

    // ---- devices + automatic driver setup ----

    fn set_devices(&mut self, items: Vec<DeviceItem>) {
        self.devices = items;
        // Devices that went away may be retried after replugging.
        let present: Vec<(u16, u16)> = self.devices.iter().map(|d| (d.vid, d.pid)).collect();
        self.driver_tried.retain(|k| present.contains(k));
        self.auto_driver();
    }

    /// Accessory-mode devices (18D1:2D00/2D01) libusb cannot open get WinUSB
    /// automatically; other Android devices only once the user pressed
    /// "Kết nối" (their driver is replaced, so that needs consent).
    fn auto_driver(&mut self) {
        if !cfg!(windows) || self.driver_busy.is_some() || self.demo.is_some() {
            return;
        }
        let cand = self.devices.iter().find(|d| {
            d.kind == "usb"
                && !d.driver_ok
                && (d.vid, d.pid) != (0, 0)
                && !self.driver_tried.contains(&(d.vid, d.pid))
                && (d.accessory || self.running)
        });
        if let Some(d) = cand.cloned() {
            self.install_driver(&d);
        }
    }

    fn install_driver(&mut self, d: &DeviceItem) {
        let (vid, pid) = (d.vid, d.pid);
        self.driver_busy = Some((vid, pid));
        self.driver_tried.insert((vid, pid));
        self.driver_note = None;
        self.log(LogLevel::Info, format!("Đang cài driver USB (WinUSB) cho {vid:04X}:{pid:04X}"));
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = driver::install_winusb(vid, pid);
            let _ = tx.send(UiMsg::Driver { vid, pid, result });
            ctx.request_repaint();
        });
    }

    fn driver_done(&mut self, vid: u16, pid: u16, result: DriverResult) {
        self.driver_busy = None;
        match result {
            DriverResult::Installed => {
                self.log(LogLevel::Info, format!("Đã cài driver USB cho {vid:04X}:{pid:04X}"));
                self.driver_note = Some((true, "Đã cài driver USB xong.".into()));
                if let Some(d) = self.connect_after_driver.take() {
                    self.connect_device(&d);
                } else {
                    self.send(HostCmd::Connect { serial: String::new() });
                }
            }
            DriverResult::Cancelled => {
                self.log(LogLevel::Warn, "Bạn đã từ chối quyền quản trị, chưa cài được driver USB".into());
                self.driver_note = Some((false, "Chưa cài driver USB vì bạn từ chối quyền quản trị. Nhấn \"Kết nối\" để thử lại.".into()));
                self.connect_after_driver = None;
            }
            DriverResult::Failed(e) => {
                self.log(LogLevel::Error, format!("Cài driver USB thất bại: {e}"));
                self.driver_note = Some((false, format!("Cài driver USB thất bại: {e}")));
                self.connect_after_driver = None;
            }
        }
    }

    /// "Kết nối" on a device card.
    fn connect_device(&mut self, d: &DeviceItem) {
        if d.kind == "usb" && !d.driver_ok && cfg!(windows) {
            if self.driver_busy.is_none() {
                self.connect_after_driver = Some(DeviceItem { driver_ok: true, ..d.clone() });
                self.install_driver(d);
            }
            return;
        }
        if !self.running {
            let want = if d.kind == "adb" { Mode::Adb } else { Mode::Usb };
            if self.cfg.mode == Mode::Lan || self.cfg.mode != want {
                self.cfg.mode = want;
                self.cfg.save();
            }
            self.start();
        }
        self.send(HostCmd::Connect { serial: d.serial.clone() });
    }

    fn device_status(&self, d: &DeviceItem) -> (Color32, String) {
        let streaming = self.view() == View::Streaming;
        if self.driver_busy == Some((d.vid, d.pid)) {
            return (Color32::from_rgb(235, 170, 40), "Đang cài driver USB (cần quyền quản trị)…".into());
        }
        if d.connected && streaming {
            return (Color32::from_rgb(40, 190, 110), "đang truyền màn hình".into());
        }
        if d.connected {
            return (Color32::from_rgb(235, 170, 40), "đã kết nối, đang chuẩn bị…".into());
        }
        if !d.driver_ok && cfg!(windows) {
            return (Color32::from_rgb(235, 170, 40), "đã cắm USB, cần cài driver (tự động khi nhấn Kết nối)".into());
        }
        if d.accessory {
            return (Color32::from_rgb(235, 170, 40), "đang ghép nối qua USB…".into());
        }
        let how = if d.kind == "adb" { "ADB" } else { "USB" };
        (accent(), format!("đã cắm {how}"))
    }

    fn tick_demo(&mut self) {
        let Some(d) = &mut self.demo else { return };
        let el = d.started.elapsed().as_secs_f32();
        let mk = |state, msg: &str| {
            HostEvent::Status(StatusEvent {
                state,
                link: Link::Usb,
                peer: Some("Galaxy Tab S10+".into()),
                otp: Some("482915".into()),
                message: Some(msg.into()),
            })
        };
        let mut evs = Vec::new();
        if d.tick == 0 {
            d.tick = 1;
            evs.push(HostEvent::Status(StatusEvent {
                state: State::Waiting,
                link: Link::None,
                peer: None,
                otp: Some("482915".into()),
                message: Some("Đang chờ thiết bị".into()),
            }));
            evs.push(HostEvent::Devices {
                items: vec![DeviceItem {
                    kind: "usb".into(),
                    name: "Galaxy Tab S10+ (SM-X826B)".into(),
                    serial: "1-5".into(),
                    vid: 0x04E8,
                    pid: 0x6860,
                    accessory: false,
                    driver_ok: true,
                    connected: false,
                }],
            });
        }
        if d.tick == 1 && el > 2.0 {
            d.tick = 2;
            evs.push(mk(State::Connecting, "Đang bắt tay mã hóa"));
        }
        if d.tick == 2 && el > 3.5 {
            d.tick = 3;
            evs.push(mk(State::Streaming, "Đang truyền"));
        }
        if d.tick == 3 && d.last_stats.elapsed() > Duration::from_millis(500) {
            d.last_stats = Instant::now();
            let w = (el * 2.0).sin();
            evs.push(HostEvent::Stats(StatsEvent {
                fps: 59.0 + w * 1.5 - if (el as u32).is_multiple_of(17) { 8.0 } else { 0.0 },
                bitrate_kbps: (24000.0 + 4000.0 * (el * 0.7).sin()) as u32,
                encode_ms: 3.4 + 0.5 * w,
                rtt_ms: Some(7.0 + 2.0 * (el * 1.3).cos()),
                decode_ms: Some(2.1 + 0.3 * f64::from(w.abs())),
                encoder: "NVENC HEVC".into(),
                width: 2560,
                height: 1600,
                fps_now: 59.0 + w * 1.5,
                fps_avg: 58.6,
                fps_low1: 44.0 + w * 2.0,
                frame_ms_mean: 17.1,
                frame_ms_std: 2.8 + 0.4 * w.abs(),
                frame_ms_var: 7.8 + 2.0 * w.abs(),
            }));
        }
        for e in evs {
            self.handle_event(e);
        }
        self.ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn view(&self) -> View {
        if !self.running {
            return if self.error.is_some() { View::Error } else { View::Idle };
        }
        match &self.status {
            None => View::Starting,
            Some(s) => match s.state {
                State::Starting => View::Starting,
                State::Waiting => View::Waiting,
                State::Connecting => View::Connecting,
                State::Connected | State::Streaming => View::Streaming,
                State::Error => View::Error,
            },
        }
    }

    // ---- screens ----

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(8), accent());
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "R", egui::FontId::proportional(18.0), Color32::WHITE);
            ui.label(RichText::new("RemoteView").size(20.0).strong());
        });
        ui.add_space(14.0);
        for (p, label) in [
            (Page::Home, "Trang chính"),
            (Page::Settings, "Cài đặt"),
            (Page::Guide, "Hướng dẫn USB"),
            (Page::Logs, "Nhật ký"),
        ] {
            let sel = self.page == p;
            let b = egui::Button::new(RichText::new(label).size(16.0).color(if sel { Color32::WHITE } else { ui.visuals().text_color() }))
                .fill(if sel { accent() } else { Color32::TRANSPARENT })
                .stroke(Stroke::NONE)
                .min_size(Vec2::new(ui.available_width(), 38.0));
            if ui.add(b).clicked() {
                self.page = p;
            }
        }
        ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
            ui.add_space(6.0);
            ui.weak(format!("Phiên bản {}", env!("CARGO_PKG_VERSION")));
            if ui.button("Thoát").clicked() {
                self.really_quit = true;
            }
            if self.demo.is_some() {
                ui.colored_label(Color32::from_rgb(230, 160, 40), "Chế độ demo");
            }
        });
    }

    fn status_card(&mut self, ui: &mut egui::Ui) {
        let v = self.view();
        let (color, title, sub) = match v {
            View::Idle => (Color32::from_rgb(130, 135, 150), "Chưa chia sẻ", "Nhấn \"Bắt đầu chia sẻ\" để bắt đầu".to_string()),
            View::Starting => (Color32::from_rgb(235, 170, 40), "Đang khởi động", "Đang chạy tiến trình host...".into()),
            View::Waiting => (
                Color32::from_rgb(52, 120, 246),
                "Chờ kết nối",
                if self.cfg.mode == Mode::Usb {
                    "Cắm cáp USB; thiết bị tự được nhận diện, không cần nhập mã".into()
                } else {
                    "Mở ứng dụng trên thiết bị nhận và nhập mã PIN".into()
                },
            ),
            View::Connecting => (Color32::from_rgb(235, 170, 40), "Đang kết nối", "Đang thiết lập kết nối an toàn...".into()),
            View::Streaming => (
                Color32::from_rgb(40, 190, 110),
                "Đang truyền",
                self.status.as_ref().and_then(|s| s.peer.clone()).map(|p| format!("Đang chia sẻ tới {p}")).unwrap_or_else(|| "Đang chia sẻ màn hình".into()),
            ),
            View::Error => (
                Color32::from_rgb(230, 70, 70),
                "Lỗi",
                self.error.clone().unwrap_or_else(|| "Đã xảy ra lỗi".into()),
            ),
        };
        card(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(64.0), egui::Sense::hover());
                let c = r.center();
                let t = ui.input(|i| i.time) as f32;
                let pulsing = matches!(v, View::Starting | View::Waiting | View::Connecting);
                if pulsing {
                    let k = (t * 2.0).sin() * 0.5 + 0.5;
                    ui.painter().circle_filled(c, 22.0 + 9.0 * k, color.gamma_multiply(0.18));
                    ui.ctx().request_repaint_after(Duration::from_millis(33));
                } else {
                    ui.painter().circle_filled(c, 30.0, color.gamma_multiply(0.18));
                }
                ui.painter().circle_filled(c, 20.0, color);
                let w = Stroke::new(3.5_f32, Color32::WHITE);
                match v {
                    View::Streaming => {
                        ui.painter().line_segment([c + Vec2::new(-8.0, 0.0), c + Vec2::new(-2.0, 7.0)], w);
                        ui.painter().line_segment([c + Vec2::new(-2.0, 7.0), c + Vec2::new(9.0, -7.0)], w);
                    }
                    View::Error => {
                        ui.painter().line_segment([c + Vec2::new(0.0, -9.0), c + Vec2::new(0.0, 2.0)], w);
                        ui.painter().circle_filled(c + Vec2::new(0.0, 8.0), 2.5, Color32::WHITE);
                    }
                    View::Idle => {
                        ui.painter().rect_filled(egui::Rect::from_center_size(c, Vec2::splat(12.0)), CornerRadius::same(2), Color32::WHITE);
                    }
                    _ => {
                        ui.painter().circle_stroke(c, 8.0, w);
                    }
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new(title).size(28.0).strong().color(color));
                    ui.label(RichText::new(sub).size(16.0));
                });
            });
        });
    }

    fn mode_selector(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(RichText::new("Kiểu kết nối").strong());
            let enabled = !self.running;
            ui.add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 20.0) / 3.0;
                    for (m, label) in [(Mode::Usb, "USB"), (Mode::Lan, "Wi-Fi / LAN"), (Mode::Adb, "ADB")] {
                        let sel = self.cfg.mode == m;
                        let b = egui::Button::new(RichText::new(label).size(17.0).color(if sel { Color32::WHITE } else { ui.visuals().text_color() }))
                            .fill(if sel { accent() } else { ui.visuals().widgets.inactive.weak_bg_fill })
                            .min_size(Vec2::new(w, 40.0));
                        if ui.add(b).clicked() && !sel {
                            self.cfg.mode = m;
                            self.cfg.save();
                        }
                    }
                });
            });
            let hint = match self.cfg.mode {
                Mode::Usb => "Cắm cáp USB, độ trễ thấp nhất. Thiết bị tự được nhận diện, không cần nhập mã; driver USB tự cài ở lần đầu.",
                Mode::Lan => "Cùng mạng Wi-Fi/LAN với thiết bị nhận",
                Mode::Adb => "Qua adb reverse, cần bật Gỡ lỗi USB trên thiết bị",
            };
            ui.weak(hint);
            if self.running {
                ui.weak("Dừng chia sẻ để đổi kiểu kết nối.");
            }
        });
    }

    fn pin_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(RichText::new("Mã PIN ghép nối").strong());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    let text = match &self.pin {
                        Some(p) if p.len() == 6 => format!("{} {}", &p[..3], &p[3..]),
                        Some(p) => p.clone(),
                        None => "--- ---".into(),
                    };
                    ui.label(RichText::new(text).size(46.0).strong().monospace());
                    ui.add_enabled_ui(self.running, |ui| {
                        if ui.button("Tạo mã mới").clicked() {
                            if self.demo.is_some() {
                                self.pin = Some(format!("{:06}", (self.t0.elapsed().as_millis() * 7919) % 1_000_000));
                            } else {
                                self.send(HostCmd::NewOtp);
                            }
                        }
                    });
                    ui.weak("Nhập mã này trên thiết bị nhận");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if let Some(p) = &self.pin {
                        let payload = util::qr_payload(p, &self.local_ip, util::port_for(self.cfg.mode), self.cfg.mode);
                        draw_qr(ui, &payload, 150.0);
                    }
                });
            });
        });
    }

    fn devices_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.label(RichText::new("Thiết bị phát hiện").strong());
            if self.devices.is_empty() {
                ui.weak("Chưa thấy thiết bị nào. Hãy cắm cáp USB hoặc mở ứng dụng trên thiết bị nhận.");
            }
            if let Some((ok, m)) = &self.driver_note {
                let c = if *ok { Color32::from_rgb(40, 190, 110) } else { Color32::from_rgb(230, 160, 40) };
                ui.colored_label(c, m);
            }
            let mut clicked: Option<DeviceItem> = None;
            let devices = self.devices.clone();
            for d in &devices {
                let (color, status) = self.device_status(d);
                let busy = self.driver_busy.is_some();
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.painter().circle_filled(ui.cursor().left_center() + Vec2::new(6.0, 0.0), 6.0, color);
                            ui.add_space(18.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&d.name).size(17.0).strong());
                                ui.label(RichText::new(status).color(color));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let label = if d.connected { "Đã kết nối" } else { "Kết nối" };
                                let b = egui::Button::new(RichText::new(label).strong().color(Color32::WHITE))
                                    .fill(accent())
                                    .min_size(Vec2::new(110.0, 36.0));
                                if ui.add_enabled(!d.connected && !busy, b).clicked() {
                                    clicked = Some(d.clone());
                                }
                            });
                        });
                    });
            }
            if let Some(d) = clicked {
                self.connect_device(&d);
            }
        });
    }

    fn stats_strip(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            let s = &self.stats;
            let stat = |ui: &mut egui::Ui, label: &str, value: String, color: Option<Color32>| {
                ui.vertical(|ui| {
                    ui.set_min_width(96.0);
                    let mut t = RichText::new(value).size(22.0).strong();
                    if let Some(c) = color {
                        t = t.color(c);
                    }
                    ui.label(t);
                    ui.weak(label);
                });
            };
            let have = s.fps_avg > 0.0;
            let na = || "--".to_string();
            let target = effective_target_fps(self.cfg.fps, &s.encoder);
            ui.horizontal_wrapped(|ui| {
                stat(ui, "FPS hiện tại", if have { format!("{:.0}", s.fps_now) } else { format!("{:.0}", s.fps) }, None);
                stat(ui, "FPS trung bình", if have { format!("{:.1}", s.fps_avg) } else { na() }, None);
                stat(ui, "FPS 1% thấp", if have { format!("{:.1}", s.fps_low1) } else { na() }, if have { Some(low1_color(s.fps_low1, target)) } else { None });
                stat(ui, "Thời gian khung (ms)", if have { format!("{:.1} ± {:.1}", s.frame_ms_mean, s.frame_ms_std) } else { na() }, None);
                stat(ui, "Phương sai (ms²)", if have { format!("{:.2}", s.frame_ms_var) } else { na() }, None);
            });
            ui.horizontal_wrapped(|ui| {
                stat(ui, "Bitrate", format!("{:.1} Mbps", s.bitrate_kbps as f32 / 1000.0), None);
                stat(ui, "Mã hóa", format!("{:.1} ms", s.encode_ms), None);
                if let Some(d) = s.decode_ms {
                    stat(ui, "Giải mã", format!("{d:.1} ms"), None);
                }
                stat(ui, "RTT", s.rtt_ms.map(|r| format!("{r:.0} ms")).unwrap_or_else(na), None);
                stat(ui, "Bộ mã hóa", if s.encoder.is_empty() { na() } else { s.encoder.clone() }, None);
                stat(ui, "Độ phân giải", if s.width > 0 { format!("{}×{}", s.width, s.height) } else { na() }, None);
            });
            if ui.small_button("Gửi khung hình chính (IDR)").clicked() {
                self.send(HostCmd::RequestIdr);
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), egui::Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(8), ui.visuals().extreme_bg_color);
            let max = self.cfg.fps.max(30) as f32 * 1.1;
            let now = Instant::now();
            let pts: Vec<egui::Pos2> = self
                .fps_hist
                .iter()
                .map(|(t, f)| {
                    let x = 1.0 - now.duration_since(*t).as_secs_f32() / 60.0;
                    egui::pos2(rect.left() + rect.width() * x, rect.bottom() - 4.0 - (rect.height() - 8.0) * (f / max).clamp(0.0, 1.0))
                })
                .collect();
            if pts.len() > 1 {
                p.add(egui::Shape::line(pts, Stroke::new(2.0_f32, Color32::from_rgb(40, 190, 110))));
            }
            p.text(rect.left_top() + Vec2::new(8.0, 4.0), egui::Align2::LEFT_TOP, "FPS - 60 giây gần nhất", egui::FontId::proportional(12.0), ui.visuals().weak_text_color());
        });
    }

    /// Everything most people need on the main path: quality preset + start with Windows.
    fn quick_card(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            let before = self.cfg.clone();
            let cur = Preset::detect(&self.cfg);
            ui.label(RichText::new("Chất lượng").strong());
            ui.horizontal_wrapped(|ui| {
                for p in Preset::ALL {
                    let sel = cur == Some(p);
                    if ui.selectable_label(sel, p.label()).on_hover_text(p.hint()).clicked() && !sel {
                        p.apply(&mut self.cfg);
                    }
                }
                if cur.is_none() {
                    let _ = ui.selectable_label(true, "Tùy chỉnh (Cài đặt › Nâng cao)");
                }
            });
            ui.weak(match cur {
                Some(p) => p.hint().to_string(),
                None => format!("{} FPS, chất lượng {}%.", self.cfg.fps, self.cfg.quality),
            });
            if util::autostart_supported() {
                ui.checkbox(&mut self.cfg.autostart, "Tự chạy cùng Windows")
                    .on_hover_text("Màn hình laptop hỏng: bật mục này để cắm cáp USB là dùng được ngay sau khi khởi động máy.");
                if let Some(m) = &self.autostart_msg {
                    ui.colored_label(Color32::from_rgb(230, 70, 70), m);
                }
            }
            if self.cfg != before {
                self.on_cfg_changed(&before);
            }
        });
    }

    fn page_home(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            if matches!(self.upd, UpdateState::Available(_) | UpdateState::Downloading(..) | UpdateState::Inaccessible) {
                card(ui, |ui| self.update_ui(ui, true));
            }
            if self.svc_needs_refresh {
                let mut refresh = false;
                card(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(Color32::from_rgb(230, 160, 40), RichText::new("Dịch vụ RemoteView cần cập nhật để dùng bản mới.").strong());
                        if ui.add_enabled(!self.vdd_busy, egui::Button::new("Cập nhật dịch vụ")).clicked() {
                            refresh = true;
                        }
                    });
                });
                if refresh {
                    self.install_service();
                }
            }
            self.status_card(ui);
            let big = if self.running {
                egui::Button::new(RichText::new("Dừng").size(20.0).strong().color(Color32::WHITE)).fill(Color32::from_rgb(214, 62, 62))
            } else {
                egui::Button::new(RichText::new("Bắt đầu chia sẻ").size(20.0).strong().color(Color32::WHITE)).fill(accent())
            };
            if ui.add(big.min_size(Vec2::new(ui.available_width(), 52.0)).corner_radius(CornerRadius::same(12))).clicked() {
                if self.running {
                    self.stop();
                } else {
                    self.start();
                }
            }
            self.quick_card(ui);
            if self.view() == View::Streaming {
                self.stats_strip(ui);
            }
            self.devices_card(ui);
            self.mode_selector(ui);
            // USB pairs automatically; the PIN is only for LAN / adb.
            if self.cfg.mode != Mode::Usb {
                self.pin_card(ui);
            }
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        let before = self.cfg.clone();
        let mut quality_dragged = false;
        let mut quality_drag_stopped = false;
        egui::ScrollArea::vertical().show(ui, |ui| {
            heading(ui, "Cài đặt");
            card(ui, |ui| {
                ui.checkbox(&mut self.cfg.auto_share, "Tự bắt đầu chia sẻ khi mở ứng dụng")
                    .on_hover_text("Cắm cáp USB là dùng được, không cần bấm \"Bắt đầu chia sẻ\".");
            });
            card(ui, |ui| {
                egui::CollapsingHeader::new(RichText::new("Nâng cao").strong()).default_open(false).show(ui, |ui| {
                egui::Grid::new("g_video").num_columns(2).spacing([20.0, 12.0]).show(ui, |ui| {
                    ui.label("Độ phân giải");
                    let cur = RESOLUTIONS.iter().find(|(r, _)| *r == self.cfg.resolution).map(|(_, n)| *n).unwrap_or("Tùy chỉnh");
                    egui::ComboBox::from_id_salt("res").selected_text(cur).width(220.0).show_ui(ui, |ui| {
                        for (r, n) in RESOLUTIONS {
                            ui.selectable_value(&mut self.cfg.resolution, *r, *n);
                        }
                    });
                    ui.end_row();

                    ui.label("Tốc độ khung hình");
                    egui::ComboBox::from_id_salt("fps").selected_text(format!("{} FPS", self.cfg.fps)).width(220.0).show_ui(ui, |ui| {
                        for f in FPS_OPTIONS {
                            ui.selectable_value(&mut self.cfg.fps, *f, format!("{f} FPS"));
                        }
                    });
                    ui.end_row();

                    ui.label("Chất lượng");
                    let q_resp = ui.add(egui::Slider::new(&mut self.cfg.quality, 1..=100).suffix("%"));
                    quality_dragged = q_resp.dragged();
                    quality_drag_stopped = q_resp.drag_stopped();
                    ui.end_row();

                    ui.label("Codec");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.cfg.codec, Codec::Auto, "Tự động");
                        ui.selectable_value(&mut self.cfg.codec, Codec::Hevc, "HEVC");
                        ui.selectable_value(&mut self.cfg.codec, Codec::H264, "H.264");
                    });
                    ui.end_row();

                    ui.label("Bộ mã hóa");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.cfg.encoder, Encoder::Auto, "Tự động");
                        ui.selectable_value(&mut self.cfg.encoder, Encoder::Hw, "Phần cứng");
                        ui.selectable_value(&mut self.cfg.encoder, Encoder::Sw, "Phần mềm");
                    });
                    ui.end_row();

                    ui.label("Màn hình cần chia sẻ");
                    ui.horizontal(|ui| {
                        let auto = "Tự động (màn hình laptop)";
                        let cur = match &self.cfg.monitor {
                            None => auto.to_string(),
                            Some(m) => self
                                .outputs
                                .iter()
                                .find(|o| o.name.eq_ignore_ascii_case(m))
                                .map(|o| o.label())
                                .unwrap_or_else(|| format!("{m} (không còn kết nối)")),
                        };
                        egui::ComboBox::from_id_salt("monitor").selected_text(cur).width(420.0).show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.cfg.monitor, None, auto);
                            for o in &self.outputs {
                                ui.selectable_value(&mut self.cfg.monitor, Some(o.name.clone()), o.label());
                            }
                        });
                        if ui.button("Làm mới").clicked() {
                            self.refresh_outputs();
                        }
                    });
                    ui.end_row();

                    ui.label("Chế độ màn hình");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.cfg.display, Display::Virtual, "Màn hình ảo chính")
                            .on_hover_text("RemoteView tự tạo một màn hình ảo đúng độ phân giải của máy bảng và đặt làm màn hình chính. Dùng khi màn laptop hỏng/tắt, không cần TeamViewer.");
                        ui.selectable_value(&mut self.cfg.display, Display::Mirror, "Hiển thị màn hình laptop");
                        ui.selectable_value(&mut self.cfg.display, Display::Extend, "Mở rộng (màn hình phụ)");
                    });
                    ui.end_row();

                    ui.label("Driver màn hình ảo");
                    ui.horizontal(|ui| {
                        if self.vdd_present {
                            ui.colored_label(Color32::from_rgb(60, 170, 90), "Đã cài (SudoVDA)");
                        } else if !driver::vdd_bundled() {
                            ui.colored_label(Color32::from_rgb(230, 160, 40), "Bản build này không kèm driver");
                        } else {
                            ui.colored_label(Color32::from_rgb(230, 160, 40), "Chưa cài");
                        }
                        let can = cfg!(windows) && driver::vdd_bundled() && !self.vdd_busy;
                        if ui.add_enabled(can, egui::Button::new("Cài / cài lại")).clicked() {
                            self.install_vdd(false);
                        }
                    });
                    ui.end_row();

                    ui.label("Dịch vụ (màn hình khóa)");
                    ui.horizontal(|ui| {
                        if self.svc_needs_refresh {
                            ui.colored_label(Color32::from_rgb(230, 160, 40), "Cần cập nhật (dịch vụ đang chạy bản cũ / đường dẫn khác)");
                        } else if self.svc_present {
                            ui.colored_label(Color32::from_rgb(60, 170, 90), "Đã cài (xem/điều khiển được màn hình khóa)");
                        } else {
                            ui.colored_label(Color32::from_rgb(230, 160, 40), "Chưa cài (khi khóa máy sẽ không thấy gì)");
                        }
                        let btn = if self.svc_needs_refresh { "Cập nhật dịch vụ" } else { "Cài / cài lại" };
                        if ui.add_enabled(cfg!(windows) && !self.vdd_busy, egui::Button::new(btn)).clicked() {
                            self.install_service();
                        }
                    });
                    ui.end_row();
                });
                ui.weak("Chất lượng, FPS và độ phân giải áp dụng ngay khi đang chia sẻ. Codec, bộ mã hóa, màn hình chia sẻ, chế độ màn hình áp dụng ở lần bắt đầu kế tiếp. Ảnh luôn giữ đúng tỉ lệ của màn hình được chia sẻ (không bị kéo giãn).");
                });
            });
            card(ui, |ui| {
                ui.label(RichText::new("Hệ thống").strong());
                ui.add_enabled_ui(util::autostart_supported(), |ui| {
                    ui.checkbox(&mut self.cfg.autostart, "Khởi động cùng Windows");
                });
                if !util::autostart_supported() {
                    ui.weak("Chỉ khả dụng trên Windows.");
                }
                if let Some(m) = &self.autostart_msg {
                    ui.colored_label(Color32::from_rgb(230, 70, 70), m);
                }
                ui.checkbox(&mut self.cfg.minimize_to_tray, "Thu nhỏ xuống khay hệ thống khi đóng cửa sổ");
                ui.checkbox(&mut self.cfg.auto_update_check, "Tự kiểm tra cập nhật khi mở ứng dụng");
                ui.horizontal(|ui| {
                    ui.label("Đường dẫn rv-host (gỡ lỗi)");
                    let mut p = self.cfg.host_path.clone().unwrap_or_default();
                    if ui.add(egui::TextEdit::singleline(&mut p).hint_text("(mặc định: chính RemoteView.exe)").desired_width(320.0)).changed() {
                        self.cfg.host_path = if p.trim().is_empty() { None } else { Some(p) };
                    }
                });
            });
            card(ui, |ui| {
                ui.label(RichText::new("Giới thiệu").strong());
                ui.label(format!("RemoteView {} (build {})", crate::update::VERSION, crate::update::short_sha()));
                let busy = matches!(self.upd, UpdateState::Checking | UpdateState::Downloading(..));
                if ui.add_enabled(!busy, egui::Button::new("Kiểm tra cập nhật")).clicked() {
                    self.check_update(true);
                }
                self.update_ui(ui, false);
            });
        });
        let quality_changed = self.cfg.quality != before.quality;
        if quality_changed && quality_dragged {
            let wait = self.quality_debounce.schedule(Instant::now(), self.cfg.quality);
            self.ctx.request_repaint_after(wait);
            // Still apply any other config fields that changed in the same frame, without sending SetQuality yet.
            let mut before_other = before.clone();
            before_other.quality = self.cfg.quality;
            if self.cfg != before_other {
                self.on_cfg_changed(&before_other);
            }
        } else if self.cfg != before {
            if quality_changed {
                self.quality_debounce.clear();
            }
            self.on_cfg_changed(&before);
        } else if quality_drag_stopped {
            if let Some(q) = self.quality_debounce.flush_now() {
                self.cfg.save();
                self.send(HostCmd::SetQuality { quality: q });
            }
        }
    }

    /// Saves the config and pushes the settings that can change live to the running host.
    fn on_cfg_changed(&mut self, before: &Config) {
        self.cfg.save();
        if self.cfg.autostart != before.autostart {
            self.autostart_msg = util::set_autostart(self.cfg.autostart).err().filter(|_| util::autostart_supported());
        }
        if self.cfg.quality != before.quality {
            self.quality_debounce.clear();
            self.send(HostCmd::SetQuality { quality: self.cfg.quality });
        }
        if self.cfg.fps != before.fps {
            self.send(HostCmd::SetFps { fps: self.cfg.fps });
        }
        if self.cfg.resolution != before.resolution {
            let (width, height) = self.cfg.resolution.unwrap_or((0, 0));
            self.send(HostCmd::SetResolution { width, height });
        }
    }

    fn page_guide(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            heading(ui, "Kết nối USB lần đầu");
            let steps: [(&str, &str); 4] = [
                ("Cài ứng dụng RemoteView trên thiết bị Android", "Tải file APK từ trang phát hành và cài lên máy tính bảng/điện thoại."),
                ("Cắm cáp USB", "Dùng cáp dữ liệu tốt, cắm thiết bị nhận vào máy tính này. Thiết bị hiện ngay ở Trang chính với tên của nó."),
                ("Nhấn \"Kết nối\" trên thiết bị", "Lần đầu, RemoteView tự cài driver USB (WinUSB) và hỏi quyền quản trị một lần; không cần Zadig. Không phải nhập mã PIN: máy tính và thiết bị tự ghép nối qua cáp."),
                ("Mở RemoteView trên thiết bị nhận", "Nếu thiết bị hỏi quyền truy cập phụ kiện USB, chọn Đồng ý (có thể tích \"Luôn mở\"). Màn hình sẽ được truyền sang thiết bị."),
            ];
            for (i, (t, d)) in steps.iter().enumerate() {
                card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                        ui.painter().circle_filled(r.center(), 17.0, accent());
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, (i + 1).to_string(), egui::FontId::proportional(18.0), Color32::WHITE);
                        ui.vertical(|ui| {
                            ui.label(RichText::new(*t).size(17.0).strong());
                            ui.label(*d);
                            if i == 0 && ui.button("Mở trang tải APK").clicked() {
                                util::open_url(RELEASE_URL);
                            }
                        });
                    });
                });
            }
            egui::Frame::new()
                .fill(Color32::from_rgb(120, 70, 10).gamma_multiply(0.25))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(230, 160, 40)))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Cảnh báo").strong().color(Color32::from_rgb(230, 160, 40)));
                    ui.label("RemoteView chỉ cài driver WinUSB cho đúng thiết bị bạn bấm \"Kết nối\" (và chế độ phụ kiện 18D1:2D00 / 2D01). Với thiết bị đó, Windows sẽ không dùng nó làm ổ MTP/ADB nữa cho tới khi bạn gỡ driver trong Device Manager.");
                });
        });
    }

    fn page_logs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            heading(ui, "Nhật ký");
            if ui.button("Sao chép").clicked() {
                let text = self.logs.iter().map(|(t, l, m)| format!("[{}] {:?} {}", fmt_t(*t), l, m)).collect::<Vec<_>>().join("\n");
                ui.ctx().copy_text(text);
            }
            if ui.button("Xóa").clicked() {
                self.logs.clear();
            }
            let exporting = self.pending_log_export.is_some();
            if ui
                .add_enabled(!exporting, egui::Button::new("Xuất log"))
                .on_hover_text("Tự thu thập log Android (nếu đang kết nối) và nén toàn bộ log ra Desktop")
                .clicked()
            {
                self.request_export_logs();
            }
        });
        if let Some((ok, note)) = &self.export_note {
            let c = if *ok { Color32::from_rgb(60, 170, 90) } else { Color32::from_rgb(230, 70, 70) };
            ui.colored_label(c, note);
        }
        card(ui, |ui| {
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                if self.logs.is_empty() {
                    ui.weak("Chưa có nhật ký.");
                }
                for (t, l, m) in &self.logs {
                    let c = match l {
                        LogLevel::Info => ui.visuals().text_color(),
                        LogLevel::Warn => Color32::from_rgb(230, 160, 40),
                        LogLevel::Error => Color32::from_rgb(230, 70, 70),
                    };
                    ui.label(RichText::new(format!("{}  {}", fmt_t(*t), m)).monospace().size(13.0).color(c));
                }
            });
        });
    }
}

fn fmt_t(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

fn draw_qr(ui: &mut egui::Ui, data: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(10), Color32::WHITE);
    let Ok(code) = qrcode::QrCode::new(data.as_bytes()) else { return };
    let n = code.width();
    let quiet = 2.0;
    let cell = size / (n as f32 + quiet * 2.0);
    for (i, c) in code.to_colors().iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % n) as f32, (i / n) as f32);
            let min = rect.min + Vec2::new((x + quiet) * cell, (y + quiet) * cell);
            ui.painter().rect_filled(egui::Rect::from_min_size(min, Vec2::splat(cell + 0.5)), CornerRadius::ZERO, Color32::BLACK);
        }
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0_f32, Color32::from_gray(200)), StrokeKind::Inside);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.poll_tray(ctx);
        self.update_tray_tooltip();
        if std::mem::take(&mut self.minimize_first_frame) {
            if self.cfg.minimize_to_tray && self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.cfg.minimize_to_tray && !self.really_quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if self.tray.is_some() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        if self.really_quit {
            self.host = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::SidePanel::left("nav").exact_width(190.0).resizable(false).show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default().frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(Margin::same(18))).show(ctx, |ui| match self.page {
            Page::Home => self.page_home(ui),
            Page::Settings => self.page_settings(ui),
            Page::Guide => self.page_guide(ui),
            Page::Logs => self.page_logs(ui),
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.cfg.save();
        self.host = None; // Drop: stop + kill child
        self.tray = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low1_color_thresholds() {
        let g = Color32::from_rgb(52, 190, 100);
        let a = Color32::from_rgb(235, 170, 40);
        let r = Color32::from_rgb(225, 70, 70);
        assert_eq!(low1_color(54.0, 60.0), g);
        assert_eq!(low1_color(53.9, 60.0), a);
        assert_eq!(low1_color(36.0, 60.0), a);
        assert_eq!(low1_color(35.9, 60.0), r);
    }

    #[test]
    fn software_encoder_caps_target() {
        assert_eq!(effective_target_fps(120, "openh264"), 60.0);
        assert_eq!(effective_target_fps(120, "mf-hevc-hw"), 120.0);
        assert_eq!(effective_target_fps(30, "openh264"), 30.0);
    }

    #[test]
    fn quality_debounce_coalesces_and_flushes() {
        let mut d = QualityDebounce::default();
        let t0 = Instant::now();
        assert_eq!(d.poll(t0), (None, None));
        d.schedule(t0, 60);
        d.schedule(t0 + Duration::from_millis(50), 40);
        // Not due yet at +100 ms (rescheduled to +200 ms)
        let (due, rem) = d.poll(t0 + Duration::from_millis(100));
        assert_eq!(due, None);
        assert!(rem.is_some());
        // Fires latest value at +200 ms
        assert_eq!(d.poll(t0 + Duration::from_millis(200)), (Some(40), None));
        assert_eq!(d.poll(t0 + Duration::from_millis(250)), (None, None));
        // Drag release flushes immediately
        d.schedule(t0, 75);
        assert_eq!(d.flush_now(), Some(75));
        assert_eq!(d.flush_now(), None);
    }
}
