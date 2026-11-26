//! egui application: state, host event handling and screens.

use crate::config::*;
use crate::control::*;
use crate::host::{find_host_exe, new_channel, HostProcess, UiMsg};
use crate::util;
use eframe::egui::{self, Color32, CornerRadius, Margin, RichText, Stroke, StrokeKind, Vec2};
use std::collections::VecDeque;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

const RELEASE_URL: &str = "https://github.com/XclipseX/window-remote-view/releases/tag/latest-build";
const ZADIG_URL: &str = "https://zadig.akeo.ie/";
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
    local_ip: String,
    demo: Option<Demo>,
    autostart_msg: Option<String>,
    ctx: egui::Context,
    quit: bool,
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

    pub fn new(cc: &eframe::CreationContext<'_>, demo: bool) -> Self {
        Self::setup_fonts(&cc.egui_ctx);
        Self::setup_style(&cc.egui_ctx);
        let (tx, rx) = new_channel();
        let mut app = Self {
            cfg: Config::load(),
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
            local_ip: util::local_ip(),
            demo: None,
            autostart_msg: None,
            ctx: cc.egui_ctx.clone(),
            quit: false,
        };
        if demo {
            app.cfg = Config::default();
            app.start();
            app.demo.as_mut().unwrap().started -= Duration::from_secs(5);
        }
        app
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

    fn start(&mut self) {
        self.running = true;
        self.status = None;
        self.error = None;
        self.stats = StatsEvent::default();
        self.fps_hist.clear();
        self.log(LogLevel::Info, format!("Bắt đầu chia sẻ ({})", self.cfg.mode.name()));
        if self.demo.is_some() || std::env::args().any(|a| a == "--demo") {
            self.demo = Some(Demo { started: Instant::now(), last_stats: Instant::now(), tick: 0 });
            return;
        }
        let exe = find_host_exe(self.cfg.host_path.as_deref());
        let ctx = self.ctx.clone();
        self.host = Some(HostProcess::start(exe, self.cfg.host_args(), self.tx.clone(), move || {
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
        self.status = None;
        self.pin = None;
        self.devices.clear();
        self.fps_hist.clear();
    }

    fn send(&self, cmd: HostCmd) {
        if let Some(h) = &self.host {
            h.send(&cmd);
        }
    }

    fn log(&mut self, level: LogLevel, msg: String) {
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
                self.stats = s;
            }
            HostEvent::Devices { items } => self.devices = items,
            HostEvent::Log { level, msg } => self.log(level, msg),
        }
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::Event(e) => self.handle_event(e),
                UiMsg::Exited { code, restart_in } => {
                    let msg = format!(
                        "Tiến trình rv-host dừng (mã {}){}",
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
                    let msg = format!("Không chạy được rv-host: {e}");
                    self.log(LogLevel::Error, msg.clone());
                    self.host = None;
                    self.running = false;
                    self.error = Some(msg);
                }
                UiMsg::Stopped => {}
            }
        }
        self.tick_demo();
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
                items: vec![DeviceItem { kind: "usb".into(), name: "Samsung Galaxy Tab S10+".into(), serial: "R52W90ABCDE".into() }],
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
                encoder: "NVENC HEVC".into(),
                width: 2560,
                height: 1600,
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
                self.quit = true;
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
            View::Waiting => (Color32::from_rgb(52, 120, 246), "Chờ kết nối", "Mở ứng dụng trên thiết bị nhận và nhập mã PIN".into()),
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
                Mode::Usb => "Cắm cáp USB, độ trễ thấp nhất (cần cài WinUSB lần đầu - xem tab Hướng dẫn USB)",
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
            for d in &self.devices {
                ui.horizontal(|ui| {
                    let kind = match d.kind.as_str() {
                        "usb" => "USB",
                        "lan" => "LAN",
                        "adb" => "ADB",
                        k => k,
                    };
                    ui.label(RichText::new(kind).strong().color(accent()));
                    ui.label(&d.name);
                    if !d.serial.is_empty() {
                        ui.weak(&d.serial);
                    }
                });
            }
        });
    }

    fn stats_strip(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            let s = &self.stats;
            ui.horizontal_wrapped(|ui| {
                let stat = |ui: &mut egui::Ui, label: &str, value: String| {
                    ui.vertical(|ui| {
                        ui.set_min_width(92.0);
                        ui.label(RichText::new(value).size(22.0).strong());
                        ui.weak(label);
                    });
                };
                stat(ui, "FPS", format!("{:.0}", s.fps));
                stat(ui, "Bitrate", format!("{:.1} Mbps", s.bitrate_kbps as f32 / 1000.0));
                stat(ui, "Mã hóa", format!("{:.1} ms", s.encode_ms));
                stat(ui, "RTT", s.rtt_ms.map(|r| format!("{r:.0} ms")).unwrap_or_else(|| "--".into()));
                stat(ui, "Bộ mã hóa", if s.encoder.is_empty() { "--".into() } else { s.encoder.clone() });
                stat(ui, "Độ phân giải", if s.width > 0 { format!("{}×{}", s.width, s.height) } else { "--".into() });
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

    fn page_home(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
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
            if self.view() == View::Streaming {
                self.stats_strip(ui);
            }
            self.mode_selector(ui);
            self.pin_card(ui);
            self.devices_card(ui);
        });
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        let before = self.cfg.clone();
        egui::ScrollArea::vertical().show(ui, |ui| {
            heading(ui, "Cài đặt");
            card(ui, |ui| {
                ui.label(RichText::new("Hình ảnh").strong());
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
                    ui.add(egui::Slider::new(&mut self.cfg.quality, 1..=100).suffix("%"));
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

                    ui.label("Chế độ màn hình");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.cfg.display, Display::Mirror, "Nhân bản");
                        ui.selectable_value(&mut self.cfg.display, Display::Extend, "Mở rộng");
                        if self.cfg.display == Display::Extend {
                            ui.colored_label(Color32::from_rgb(230, 160, 40), "sắp có - cần driver màn hình ảo");
                        }
                    });
                    ui.end_row();
                });
                ui.weak("Chất lượng, FPS và độ phân giải áp dụng ngay khi đang chia sẻ. Codec, bộ mã hóa, chế độ màn hình áp dụng ở lần bắt đầu kế tiếp.");
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
                ui.checkbox(&mut self.cfg.minimize_to_tray, "Thu nhỏ xuống thanh tác vụ khi đóng cửa sổ");
                ui.horizontal(|ui| {
                    ui.label("Đường dẫn rv-host");
                    let mut p = self.cfg.host_path.clone().unwrap_or_default();
                    if ui.add(egui::TextEdit::singleline(&mut p).hint_text("(mặc định: cùng thư mục)").desired_width(320.0)).changed() {
                        self.cfg.host_path = if p.trim().is_empty() { None } else { Some(p) };
                    }
                });
            });
        });
        if self.cfg != before {
            self.cfg.save();
            if self.cfg.autostart != before.autostart {
                self.autostart_msg = util::set_autostart(self.cfg.autostart).err().filter(|_| util::autostart_supported());
            }
            if self.cfg.quality != before.quality {
                self.send(HostCmd::SetQuality { quality: self.cfg.quality });
            }
            if self.cfg.fps != before.fps {
                self.send(HostCmd::SetFps { fps: self.cfg.fps });
            }
            if self.cfg.resolution != before.resolution {
                if let Some((width, height)) = self.cfg.resolution {
                    self.send(HostCmd::SetResolution { width, height });
                }
            }
        }
    }

    fn page_guide(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            heading(ui, "Kết nối USB lần đầu");
            let steps: [(&str, &str); 5] = [
                ("Cài ứng dụng RemoteView trên thiết bị Android", "Tải file APK từ trang phát hành và cài lên máy tính bảng/điện thoại."),
                ("Cắm cáp USB", "Dùng cáp dữ liệu tốt, cắm thiết bị nhận vào máy tính này."),
                ("Cài driver WinUSB bằng Zadig", "Mở Zadig, chọn Options > List All Devices. Chọn thiết bị Android ở chế độ phụ kiện (ID 18D1:2D00 hoặc 18D1:2D01), driver đích WinUSB, rồi Install Driver."),
                ("Mở ứng dụng và nhập mã PIN", "Trên thiết bị nhận, mở RemoteView rồi nhập mã PIN hiển thị ở Trang chính."),
                ("Nhấn \"Bắt đầu chia sẻ\"", "Chọn kiểu USB, nhấn bắt đầu; màn hình sẽ được truyền sang thiết bị."),
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
                            if i == 2 && ui.button("Mở trang tải Zadig").clicked() {
                                util::open_url(ZADIG_URL);
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
                    ui.label("Chỉ thay driver cho thiết bị 18D1:2D00 / 18D1:2D01. KHÔNG thay driver của thiết bị Samsung (04E8), nếu không Samsung sẽ không nhận được kết nối bình thường (MTP/ADB).");
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
        });
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
        if ctx.input(|i| i.viewport().close_requested()) && self.cfg.minimize_to_tray && !self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if self.quit {
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
    }
}
