use eframe::egui;
use std::sync::mpsc::{self, Receiver};
use std::thread;

pub struct AutoSubSyncApp {
    ref_file: String,
    tgt_file: String,
    out_file: String,
    logs: Vec<String>,
    is_syncing: bool,
    log_receiver: Option<Receiver<String>>,
    progress: f32,
}

impl Default for AutoSubSyncApp {
    fn default() -> Self {
        Self {
            ref_file: String::new(),
            tgt_file: String::new(),
            out_file: String::from(""),
            logs: Vec::new(),
            is_syncing: false,
            log_receiver: None,
            progress: 0.0,
        }
    }
}

impl AutoSubSyncApp {
    fn update_output_path(&mut self) {
        if self.ref_file.is_empty() { return; }
        let path = std::path::Path::new(&self.ref_file);
        if let (Some(parent), Some(stem)) = (path.parent(), path.file_stem()) {
            let out_name = format!("{}-synced.srt", stem.to_string_lossy());
            self.out_file = parent.join(out_name).to_string_lossy().to_string();
        }
    }
}

impl eframe::App for AutoSubSyncApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                    if lmsg.contains("streaming audio") { self.progress = 0.05; }
                    else if lmsg.contains("voice activity detection") { self.progress = 0.15; }
                    else if lmsg.contains("parsing target") { self.progress = 0.35; }
                    else if lmsg.contains("correlation") || lmsg.contains("fft") { self.progress = 0.45; }
                    else if lmsg.contains("regression") { self.progress = 0.75; }
                    else if lmsg.contains("aligning") || lmsg.contains("deltas") { self.progress = 0.90; }
                    else { if self.progress < 0.9 { self.progress += 0.02; } }
                    self.logs.push(msg);
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
            let ref_bg = if !self.ref_file.is_empty() {
                if is_hovering_ref { egui::Color32::from_rgb(215, 245, 215) } else { egui::Color32::from_rgb(235, 255, 235) }
            } else {
                if is_hovering_ref { egui::Color32::from_rgb(245, 245, 255) } else { egui::Color32::WHITE }
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
                if let Some(path) = rfd::FileDialog::new().pick_file() {
                    self.ref_file = path.display().to_string();
                    self.update_output_path();
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
                if is_hovering_tgt { egui::Color32::from_rgb(245, 245, 255) } else { egui::Color32::WHITE }
            };

            ui.painter().rect_filled(tgt_box_rect, 5.0, tgt_bg);
            draw_dashed_rect(ui.painter(), tgt_box_rect, egui::Color32::from_rgb(180, 180, 180), 2.0);

            ui.put(tgt_box_rect, egui::Label::new(
                egui::RichText::new(if self.tgt_file.is_empty() { "Input Subtitle\n\nDrag and drop or click to browse" } else { &self.tgt_file })
                .color(text_color)
                .size(14.0)
            ).selectable(false));

            if tgt_box_response.clicked() {
                if let Some(path) = rfd::FileDialog::new().add_filter("Subtitles", &["srt", "ass", "ssa", "vtt"]).pick_file() {
                    self.tgt_file = path.display().to_string();
                    self.update_output_path();
                }
            }

            ui.add_space(20.0);

            let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
            if !dropped_files.is_empty() {
                for file in dropped_files {
                    if let Some(path) = file.path {
                        let path_str = path.display().to_string();
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

                        if ext == "srt" || ext == "vtt" || ext == "ass" || ext == "ssa" {
                            self.tgt_file = path_str;
                            self.update_output_path();
                            self.logs.push(format!("Auto-Routed {} to Subtitle", ext));
                        } else {
                            self.ref_file = path_str;
                            self.update_output_path();
                            self.logs.push(format!("Auto-Routed {} to Video", ext));
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
                        if self.ref_file.is_empty() || self.tgt_file.is_empty() {
                            self.logs.push("Please select both files.".to_string());
                        } else {
                            self.is_syncing = true;
                            self.progress = 0.0;
                            self.logs.clear();
                            let (tx, rx) = mpsc::channel();
                            self.log_receiver = Some(rx);

                            let ref_path = self.ref_file.clone();
                            let tgt_path = self.tgt_file.clone();
                            let out_path = self.out_file.clone();
                            let ctx_clone = ctx.clone();

                            thread::spawn(move || {
                                let tx_inner = tx.clone();
                                let tx_cb = tx.clone();
                                let ctx_inner = ctx_clone.clone();
                                let ctx_cb = ctx_clone.clone();

                                let result = crate::sync::run_sync(&ref_path, &tgt_path, &out_path, move |msg| {
                                    let _ = tx_cb.send(msg);
                                    ctx_cb.request_repaint();
                                });

                                if let Err(e) = result {
                                    let _ = tx_inner.send(format!("###SYNC_ERROR###{}", e));
                                } else {
                                    let _ = tx_inner.send("###SYNC_COMPLETE###".to_string());
                                }
                                ctx_inner.request_repaint();
                            });
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
