#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! RemoteView - giao diện Windows (host) cho ứng dụng chia sẻ màn hình độ trễ thấp.

mod app;
mod config;
mod control;
#[cfg_attr(not(windows), allow(dead_code))]
mod driver;
mod host;
mod logexport;
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
    init_logging();
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

/// Starts the log file (`%LOCALAPPDATA%\\RemoteView\\logs\\remoteview-<date-time>.log`) from app
/// start and records the environment (version, commit, OS, GPUs, outputs).
fn init_logging() {
    use rv_host_win::logfile::{init, write_line, PREFIX};
    let path = init(PREFIX);
    std::panic::set_hook(Box::new(|info| write_line("error", &format!("PANIC: {info}"))));
    for l in rv_host_win::sysinfo::header_lines("RemoteView GUI") {
        write_line("info", &l);
    }
    write_line("info", &format!("build sha: {}", update::BUILD_SHA));
    match &path {
        Some(p) => write_line("info", &format!("log file: {}", p.display())),
        None => eprintln!("cannot create a log file in {}", rv_host_win::logfile::logs_dir().display()),
    }
    // GPU / output enumeration can take a moment: do it off the UI thread.
    std::thread::spawn(|| {
        for l in rv_host_win::sysinfo::startup_report("RemoteView GUI").into_iter().skip(3) {
            write_line("info", &l);
        }
    });
}
