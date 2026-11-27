#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! RemoteView - giao diện Windows (host) cho ứng dụng chia sẻ màn hình độ trễ thấp.

mod app;
mod config;
mod control;
#[cfg_attr(not(windows), allow(dead_code))]
mod driver;
mod host;
mod logexport;
mod service;
mod update;
mod util;

fn main() -> eframe::Result {
    // Single-executable layout: the host engine runs as this same exe in worker
    // mode (JSON lines on stdin/stdout), so no separate rv-host.exe is shipped.
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some(host::WORKER_FLAG) => {
            let code = rv_host_win::engine::run_cli(args.collect());
            std::process::exit(code);
        }
        // Elevated helper mode (started by the GUI through UAC): install SudoVDA from <dir>.
        Some(driver::INSTALL_VDD_FLAG) => std::process::exit(install_vdd_main(args.next())),
        // Elevated: SudoVDA (+ the RemoteView service, so the lock screen can be shown/controlled).
        Some(service::INSTALL_ALL_FLAG) => std::process::exit(service::install_all_main(args.next())),
        Some(service::UNINSTALL_SERVICE_FLAG) => std::process::exit(service::uninstall_main()),
        // Started by the Windows service manager: runs the host engine as SYSTEM on request.
        Some(service::SERVICE_FLAG) => std::process::exit(service::run_service()),
        _ => {}
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

#[cfg(windows)]
fn install_vdd_main(dir: Option<String>) -> i32 {
    use rv_host_win::logfile::{init, write_line, PREFIX};
    let _ = init(PREFIX);
    let Some(dir) = dir else { return 2 };
    write_line("info", &format!("install-vdd: installing SudoVDA from {dir}"));
    match rv_host_win::sudovda::install(std::path::Path::new(&dir)) {
        Ok(()) => {
            write_line("info", "install-vdd: done");
            0
        }
        Err(e) => {
            write_line("error", &format!("install-vdd: {e}"));
            1
        }
    }
}

#[cfg(not(windows))]
fn install_vdd_main(_dir: Option<String>) -> i32 {
    1
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
