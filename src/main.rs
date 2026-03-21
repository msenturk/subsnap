#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod config;
mod correlation;
mod regression;
mod srt;
mod sync;
mod ui;
mod vad;

use eframe::egui;

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([600.0, 800.0])
            .with_icon(load_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "SubSnap",
        options,
        Box::new(|_cc| Ok(Box::new(ui::AutoSubSyncApp::default()))),
    )
}

fn load_icon() -> egui::IconData {
    let rgba = include_bytes!("../icon.rgba");
    egui::IconData {
        rgba: rgba.to_vec(),
        width: 64,
        height: 64,
    }
}
