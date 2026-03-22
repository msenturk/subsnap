#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ui;

use subsnap::{audio, sync, config, vad};

use eframe::egui;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
fn main() {
    // Make sure panics are logged using `console.error`.
    console_error_panic_hook::set_once();
    web_sys::console::log_1(&"SubSnap WASM v1.0.2 - Arc + Bounded Channel".into());

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let (_tx, rx) = std::sync::mpsc::channel();
        let app = ui::AutoSubSyncApp::new_with_receiver(rx);

        let window = web_sys::window().expect("no global `window` exists");
        let document = window.document().expect("should have a document on window");
        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("should have #the_canvas_id on the page")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("the_canvas_id should be a canvas");

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|_cc| Ok(Box::new(app))),
            )
            .await
            .expect("failed to start eframe");
    });
}


#[allow(dead_code)]
fn load_icon() -> egui::IconData {
    let rgba = include_bytes!("../icon.rgba");
    egui::IconData {
        rgba: rgba.to_vec(),
        width: 64,
        height: 64,
    }
}
