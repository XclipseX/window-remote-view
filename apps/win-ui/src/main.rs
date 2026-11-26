#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! RemoteView - giao diện Windows (host) cho ứng dụng chia sẻ màn hình độ trễ thấp.

mod app;
mod config;
mod control;
#[cfg_attr(not(windows), allow(dead_code))]
mod driver;
mod host;
mod update;
mod util;

fn main() -> eframe::Result {
    // Single-executable layout: the host engine runs as this same exe in worker
    // mode (JSON lines on stdin/stdout), so no separate rv-host.exe is shipped.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(host::WORKER_FLAG) {
        let code = rv_host_win::engine::run_cli(args.collect());
        std::process::exit(code);
    }
    update::cleanup_old();
    let demo = std::env::args().any(|a| a == "--demo");
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("RemoteView")
        .with_inner_size([980.0, 680.0])
        .with_min_inner_size([760.0, 560.0])
        .with_icon(eframe::egui::IconData {
            rgba: util::icon_rgba(64),
            width: 64,
            height: 64,
        });
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "RemoteView",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, demo)))),
    )
}
