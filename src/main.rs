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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        // CLI Mode
        let ref_path = &args[1];
        let tgt_path = if args.len() > 2 {
            args[2].clone()
        } else {
            // Try to find a matching SRT or VTT if only media is provided
            let path = std::path::Path::new(ref_path);
            let stem = path.file_stem().ok_or("Invalid media path")?.to_string_lossy();
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            
            let srt_path = parent.join(format!("{}.srt", stem));
            let vtt_path = parent.join(format!("{}.vtt", stem));
            
            if srt_path.exists() {
                srt_path.to_string_lossy().to_string()
            } else if vtt_path.exists() {
                vtt_path.to_string_lossy().to_string()
            } else {
                // Fallback to searching for any .srt/.vtt that starts with the stem
                let mut found = None;
                if let Ok(entries) = std::fs::read_dir(parent) {
                    for entry in entries.flatten() {
                        let fname = entry.file_name().to_string_lossy().to_lowercase();
                        if fname.starts_with(&stem.to_lowercase()) && (fname.ends_with(".srt") || fname.ends_with(".vtt")) {
                            found = Some(entry.path().to_string_lossy().to_string());
                            break;
                        }
                    }
                }
                found.ok_or_else(|| format!("Could not find a matching subtitle file for {}", stem))?
            }
        };

        let out_path = if args.len() > 3 {
            args[3].clone()
        } else {
            let path = std::path::Path::new(&tgt_path);
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            let stem = path.file_stem().ok_or("Invalid target path")?.to_string_lossy();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("srt");
            parent.join(format!("{}-synced.{}", stem, ext)).to_string_lossy().to_string()
        };

        eprintln!("SubSnap CLI Mode");
        eprintln!("Reference: {}", ref_path);
        eprintln!("Target:    {}", tgt_path);
        eprintln!("Output:    {}", out_path);

        let progress_cb = std::sync::Arc::new(move |msg: String| {
            eprintln!("[LOG] {}", msg);
        });

        let result = crate::sync::run_sync(ref_path, &tgt_path, &out_path, progress_cb);

        if let Err(e) = result {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        eprintln!("Success!");
        return Ok(());
    }

    // GUI Mode
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
    ).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

fn load_icon() -> egui::IconData {
    let rgba = include_bytes!("../icon.rgba");
    egui::IconData {
        rgba: rgba.to_vec(),
        width: 64,
        height: 64,
    }
}
