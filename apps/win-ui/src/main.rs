#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! RemoteView - giao diện Windows (host) cho ứng dụng chia sẻ màn hình độ trễ thấp.

mod app;
mod config;
mod control;
mod host;
mod util;

fn main() -> eframe::Result {
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
