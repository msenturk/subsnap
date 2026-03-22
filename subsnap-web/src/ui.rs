use eframe::egui;
use std::sync::mpsc::{self, Receiver};

use std::sync::Arc;

pub struct AutoSubSyncApp {
    ref_file: String,
    ref_data: Option<Arc<Vec<u8>>>,
    tgt_file: String,
    tgt_data: Option<Arc<Vec<u8>>>,
    out_file: String,
    logs: Vec<String>,
    is_syncing: bool,
    log_receiver: Option<Receiver<String>>,
    file_receiver: Option<Receiver<(bool, String, Arc<Vec<u8>>)>>,
    file_sender: Option<mpsc::Sender<(bool, String, Arc<Vec<u8>>)>>, 
    progress: f32,
}

impl Default for AutoSubSyncApp {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            ref_file: String::new(),
            ref_data: None,
            tgt_file: String::new(),
            tgt_data: None,
            out_file: String::from(""),
            logs: Vec::new(),
            is_syncing: false,
            log_receiver: None,
            file_receiver: Some(rx),
            file_sender: Some(tx),
            progress: 0.0,
        }
    }
}

impl AutoSubSyncApp {
    pub fn new_with_receiver(rx: Receiver<String>) -> Self {
        let mut s = Self::default();
        s.log_receiver = Some(rx);
        s
    }

    fn update_output_path(&mut self) {
        if self.ref_file.is_empty() { return; }
        let path = std::path::Path::new(&self.ref_file);
        if let Some(stem) = path.file_stem() {
            let out_name = format!("{}-synced.srt", stem.to_string_lossy());
            self.out_file = out_name;
        }
    }
}

impl eframe::App for AutoSubSyncApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut file_updates = Vec::new();
        if let Some(rx) = &self.file_receiver {
            while let Ok(update) = rx.try_recv() {
                file_updates.push(update);
            }
        }
        for (is_ref, name, data) in file_updates {
            if is_ref {
                self.ref_file = name;
                self.ref_data = Some(data);
            } else {
                self.tgt_file = name;
                self.tgt_data = Some(data);
            }
            self.update_output_path();
        }

        if let Some(rx) = &self.log_receiver {
            while let Ok(msg) = rx.try_recv() {
                if msg == "###SYNC_COMPLETE###" || msg.starts_with("###SYNC_ERROR###") {
                    self.is_syncing = false;
                    self.progress = 1.0;
                    if msg.starts_with("###SYNC_ERROR###") {
                        self.logs.push(format!("Error: {}", &msg[16..]));
                    }
                } else {
                    let lmsg = msg.to_lowercase();
                    if msg.starts_with("PROGRESS_VAD:") {
                        if let Ok(p) = msg[13..].parse::<f32>() {
                            // Map 0-100 VAD% to 0.15-0.70 total progress (VAD is the longest step)
                            self.progress = 0.15 + (p / 100.0) * 0.55;
                        }
                    } 
                    else if lmsg.contains("streaming audio") { self.progress = 0.05; }
                    else if lmsg.contains("voice activity detection") { self.progress = 0.15; }
                    else if lmsg.contains("parsing target") { self.progress = 0.35; }
                    else if lmsg.contains("correlation") || lmsg.contains("fft") { self.progress = 0.45; }
                    else if lmsg.contains("regression") { self.progress = 0.75; }
                    else if lmsg.contains("aligning") || lmsg.contains("deltas") { self.progress = 0.90; }
                    else { if self.progress < 0.9 { self.progress += 0.005; } } // Reduced creep
                    
                    if !msg.starts_with("PROGRESS_VAD:") {
                        self.logs.push(msg);
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("SubSnap");
            });
            ui.add_space(20.0);

            // Box 1: Reference
            let ref_box_rect = ui.allocate_space(egui::vec2(ui.available_width(), 120.0)).1;
            let ref_box_response = ui.interact(ref_box_rect, ui.id().with("ref_box"), egui::Sense::click());

            let is_hovering_ref = ui.rect_contains_pointer(ref_box_rect);
            let is_hovering_any_file = ctx.input(|i| !i.raw.hovered_files.is_empty());
            
            let ref_bg = if !self.ref_file.is_empty() {
                if is_hovering_ref { egui::Color32::from_rgb(215, 245, 215) } else { egui::Color32::from_rgb(235, 255, 235) }
            } else {
                if is_hovering_ref && is_hovering_any_file { 
                    egui::Color32::from_rgb(200, 220, 255) 
                } else if is_hovering_ref { 
                    egui::Color32::from_rgb(245, 245, 255) 
                } else if is_hovering_any_file {
                    egui::Color32::from_rgb(250, 250, 255)
                } else { 
                    egui::Color32::WHITE 
                }
            };
            let text_color = egui::Color32::from_rgb(40, 40, 40);

            ui.painter().rect_filled(ref_box_rect, 5.0, ref_bg);
            draw_dashed_rect(ui.painter(), ref_box_rect, egui::Color32::from_rgb(180, 180, 180), 2.0);

            ui.put(ref_box_rect, egui::Label::new(
                egui::RichText::new(if self.ref_file.is_empty() { "Video/Reference Subtitle\n\nDrag and drop or click to browse" } else { &self.ref_file })
                .color(text_color)
                .size(14.0)
            ).selectable(false));

            if ref_box_response.clicked() {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        self.ref_file = path.display().to_string();
                        self.ref_data = std::fs::read(&path).ok().map(Arc::new);
                        self.update_output_path();
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    let ctx_ui = ui.ctx().clone();
                    if let Some(tx) = &self.file_sender {
                        let tx_clone = tx.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Some(file) = rfd::AsyncFileDialog::new().pick_file().await {
                                let name = file.file_name();
                                let data = file.read().await;
                                let _ = tx_clone.send((true, name, Arc::new(data)));
                                ctx_ui.request_repaint();
                            }
                        });
                    }
                }
            }

            ui.add_space(15.0);

            // Box 2: Target Subtitle
            let tgt_box_rect = ui.allocate_space(egui::vec2(ui.available_width(), 120.0)).1;
            let tgt_box_response = ui.interact(tgt_box_rect, ui.id().with("tgt_box"), egui::Sense::click());

            let is_hovering_tgt = ui.rect_contains_pointer(tgt_box_rect);
            let tgt_bg = if !self.tgt_file.is_empty() {
                if is_hovering_tgt { egui::Color32::from_rgb(215, 245, 215) } else { egui::Color32::from_rgb(235, 255, 235) }
            } else {
                if is_hovering_tgt && is_hovering_any_file { 
                    egui::Color32::from_rgb(200, 220, 255) 
                } else if is_hovering_tgt { 
                    egui::Color32::from_rgb(245, 245, 255) 
                } else if is_hovering_any_file {
                    egui::Color32::from_rgb(250, 250, 255)
                } else { 
                    egui::Color32::WHITE 
                }
            };

            ui.painter().rect_filled(tgt_box_rect, 5.0, tgt_bg);
            draw_dashed_rect(ui.painter(), tgt_box_rect, egui::Color32::from_rgb(180, 180, 180), 2.0);

            ui.put(tgt_box_rect, egui::Label::new(
                egui::RichText::new(if self.tgt_file.is_empty() { "Input Subtitle\n\nDrag and drop or click to browse" } else { &self.tgt_file })
                .color(text_color)
                .size(14.0)
            ).selectable(false));

            if tgt_box_response.clicked() {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Subtitles", &["srt", "ass", "ssa", "vtt"]).pick_file() {
                        self.tgt_file = path.display().to_string();
                        self.tgt_data = std::fs::read(&path).ok().map(Arc::new);
                        self.update_output_path();
                    }
                }
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some(tx) = &self.file_sender {
                        let tx_clone = tx.clone();
                        let ctx_upload = ui.ctx().clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Some(file) = rfd::AsyncFileDialog::new().add_filter("Subtitles", &["srt", "ass", "ssa", "vtt"]).pick_file().await {
                                let name = file.file_name();
                                let data = file.read().await;
                                let _ = tx_clone.send((false, name, Arc::new(data)));
                                ctx_upload.request_repaint();
                            }
                        });
                    }
                }
            }

            ui.add_space(20.0);

            // Handle dropped files
            let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped_files.is_empty() {
                for file in dropped_files {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        if let Some(path) = file.path {
                            let path_str = path.display().to_string();
                            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                            let data = std::fs::read(&path).ok();

                            if ext == "srt" || ext == "vtt" || ext == "ass" || ext == "ssa" {
                                self.tgt_file = path_str;
                                self.tgt_data = data.map(Arc::new);
                                self.update_output_path();
                            } else {
                                self.ref_file = path_str;
                                self.ref_data = data.map(Arc::new);
                                self.update_output_path();
                            }
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        let name = file.name.clone();
                        let ext = std::path::Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let is_ref = !(ext == "srt" || ext == "vtt" || ext == "ass" || ext == "ssa");
                        
                        if let Some(tx) = &self.file_sender {
                            let tx_clone = tx.clone();
                            let ctx_clone = ctx.clone();
                            
                            // On web, we might have bytes already if egui provided them
                            if let Some(bytes) = file.bytes {
                                let _ = tx_clone.send((is_ref, name, Arc::new(bytes.to_vec())));
                                ctx_clone.request_repaint();
                            } else {
                                // If not, we might need to use rfd or web_sys if possible, 
                                // but usually egui web provides bytes if the browser does.
                                // If file.bytes is None, it's hard to get data without a File object.
                            }
                        }
                    }
                }
            }

            ui.horizontal(|ui| {
                ui.label("Output Name:");
                ui.text_edit_singleline(&mut self.out_file);
            });

            ui.add_space(10.0);

            ui.add_enabled_ui(!self.is_syncing, |ui| {
                ui.horizontal(|ui| {
                    let start_btn = egui::Button::new(egui::RichText::new("Start Synchronization").size(16.0).strong());
                    if ui.add_sized([ui.available_width() - 100.0, 40.0], start_btn).clicked() {
                        if self.ref_data.is_none() || self.tgt_data.is_none() {
                            self.logs.push("Please select both files (and wait for upload if on web).".to_string());
                        } else {
                            self.is_syncing = true;
                            self.progress = 0.0;
                            self.logs.clear();
                            let (tx, rx) = mpsc::channel();
                            self.log_receiver = Some(rx);

                            let ref_name = self.ref_file.clone();
                            let ref_data = self.ref_data.clone().unwrap(); // Small Arc clone
                            let tgt_name = self.tgt_file.clone();
                            let tgt_data = self.tgt_data.clone().unwrap(); // Small Arc clone
                            let out_path = self.out_file.clone();
                            let ctx_clone = ctx.clone();

                                #[cfg(not(target_arch = "wasm32"))]
                                std::thread::spawn(move || {
                                    let tx_inner = tx.clone();
                                    let tx_cb = tx.clone();
                                    let ctx_inner = ctx_clone.clone();
                                    let ctx_cb = ctx_clone.clone();

                                    let result = pollster::block_on(subsnap::sync::run_sync_data(ref_data, &ref_name, tgt_data, &tgt_name, move |msg| {
                                        let _ = tx_cb.send(msg);
                                        ctx_cb.request_repaint();
                                    }));

                                match result {
                                    Ok(synced_content) => {
                                        let _ = std::fs::write(&out_path, synced_content);
                                        let _ = tx_inner.send("###SYNC_COMPLETE###".to_string());
                                    }
                                    Err(e) => {
                                        let _ = tx_inner.send(format!("###SYNC_ERROR###{}", e));
                                    }
                                }
                                ctx_inner.request_repaint();
                            });

                            #[cfg(target_arch = "wasm32")]
                            {
                                // In WASM we run it in a "local" task. This will block UI but it's the simplest start.
                                wasm_bindgen_futures::spawn_local(async move {
                                    let tx_inner = tx.clone();
                                    let tx_cb = tx.clone();
                                    let ctx_cb = ctx_clone.clone();
                                    let ctx_cb_inner = ctx_clone.clone();

                                    // Yield one browser frame to ensure the UI updates (disabled button + Starting sync...)
                                    #[cfg(target_arch = "wasm32")]
                                    subsnap::sync::yield_now().await;

                                    web_sys::console::log_1(&"Starting sync task core...".into());
                                    let result = subsnap::sync::run_sync_data(ref_data, &ref_name, tgt_data, &tgt_name, move |msg| {
                                        web_sys::console::log_1(&msg.clone().into());
                                        let _ = tx_cb.send(msg);
                                        ctx_cb_inner.request_repaint();
                                    }).await;
                                    web_sys::console::log_1(&"Sync task core finished!".into());

                                    match result {
                                        Ok(synced_content) => {
                                            // Trigger download in browser
                                            download_file(&out_path, &synced_content);
                                            let _ = tx_inner.send("###SYNC_COMPLETE###".to_string());
                                        }
                                        Err(e) => {
                                            let _ = tx_inner.send(format!("###SYNC_ERROR###{}", e));
                                        }
                                    }
                                    ctx_cb.request_repaint();
                                });
                            }
                        }
                    }

                    let clear_btn = egui::Button::new(egui::RichText::new("Clear").size(16.0));
                    if ui.add_sized([ui.available_width(), 40.0], clear_btn).clicked() {
                        self.ref_file.clear();
                        self.tgt_file.clear();
                        self.out_file = String::from("");
                        self.logs.clear();
                        self.progress = 0.0;
                    }
                });
            });

            ui.add_space(10.0);
            ui.separator();
            ui.heading("Activity Log");

            if self.is_syncing {
                ui.add_space(5.0);
                let percent = (self.progress * 100.0) as i32;
                ui.add(egui::ProgressBar::new(self.progress)
                    .animate(self.progress < 1.0)
                    .text(format!("Processing... {}%", percent)));

                ctx.request_repaint();
                ui.add_space(5.0);
            }

            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
                for log in &self.logs {
                    ui.label(log);
                }
            });
        });
    }
}

fn draw_dashed_rect(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32, thickness: f32) {
    let dash_length = 6.0;
    let gap_length = 4.0;
    let stroke = egui::Stroke::new(thickness, color);

    let draw_dashed_line = |p1: egui::Pos2, p2: egui::Pos2| {
        let diff = p2 - p1;
        let length = diff.length();
        let dir = diff / length;
        let mut t = 0.0;
        while t < length {
            let start = p1 + dir * t;
            let end = p1 + dir * (t + dash_length).min(length);
            painter.line_segment([start, end], stroke);
            t += dash_length + gap_length;
        }
    };

    draw_dashed_line(rect.left_top(), rect.right_top());
    draw_dashed_line(rect.right_top(), rect.right_bottom());
    draw_dashed_line(rect.right_bottom(), rect.left_bottom());
    draw_dashed_line(rect.left_bottom(), rect.left_top());
}

#[cfg(target_arch = "wasm32")]
fn download_file(name: &str, content: &str) {
    use wasm_bindgen::JsCast;
    let window = web_sys::window().unwrap();
    let document = window.document().unwrap();
    let blob = web_sys::Blob::new_with_u8_array_sequence(&js_sys::Array::of1(&js_sys::Uint8Array::from(content.as_bytes())))
        .unwrap();
    let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();
    
    let a = document.create_element("a").unwrap().dyn_into::<web_sys::HtmlAnchorElement>().unwrap();
    a.set_href(&url);
    a.set_download(name);
    a.click();
    web_sys::Url::revoke_object_url(&url).unwrap();
}

impl AutoSubSyncApp {}
