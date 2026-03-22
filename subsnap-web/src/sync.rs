use std::sync::Arc;
use crate::{audio, srt};
use alass_core::{align, standard_scoring, NoProgressHandler, TimeSpan, TimePoint};

macro_rules! log {
    ($($t:tt)*) => {
        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&format!($($t)*).into());
        #[cfg(not(target_arch = "wasm32"))]
        println!($($t)*);
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn yield_now() {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
        } else {
            let _ = resolve.call0(&JsValue::UNDEFINED);
        }
    });
    let _ = JsFuture::from(promise).await;
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn yield_now() {} // No-op on native

pub async fn run_sync_data(
    ref_data: Arc<Vec<u8>>,
    ref_name: &str,
    tgt_data: Arc<Vec<u8>>,
    tgt_name: &str,
    mut progress_callback: impl FnMut(String),
) -> Result<String, String> {
    let config = crate::config::SyncConfig::default();
    progress_callback(format!("Starting sync..."));

    let lower_path = ref_name.to_lowercase();
    let is_media = lower_path.ends_with(".mp4") || lower_path.ends_with(".wav") || lower_path.ends_with(".mkv") || lower_path.ends_with(".avi");
    
    let (ref_timespans, ref_energy) = if is_media {
        progress_callback(format!("Streaming audio from {}...", ref_name));
        yield_now().await;

        // Final production pipeline: Decoding + VAD in one streaming pass to save RAM
        progress_callback("Decoding & Analyzing audio...".to_string());
        yield_now().await;

        let (ref_timespans, ref_energy, _sample_rate) = audio::decode_all_to_memory(
            ref_data,
            ref_name,
            config.clone(),
            |msg| progress_callback(msg),
        ).await.map_err(|e| format!("Audio sync failed: {}", e))?;

        (ref_timespans, ref_energy)
    } else {
        progress_callback(format!("Parsing reference: {}", ref_name));
        let ref_blocks = srt::parse_subtitle_data(&ref_data, ref_name)?;
        (srt::blocks_to_timespans(&ref_blocks), Vec::new())
    };
 
    if ref_timespans.len() > 5 {
        progress_callback(format!("VAD: Found {} segments. Pattern-Seeker active.", ref_timespans.len()));
        progress_callback(format!("VAD Trace: First 5 segments: {:?}", &ref_timespans[0..5]));
    }

    progress_callback("Step 2: Parsing Target Subtitles...".to_string());
    let tgt_blocks_orig = srt::parse_subtitle_data(&tgt_data, tgt_name)?;
    let tgt_timespans = srt::blocks_to_timespans(&tgt_blocks_orig);

    let duration_ms = ref_timespans.last().map(|ts| ts.end.as_i64()).unwrap_or(0);
    
    progress_callback("Step 3: Calculating Global Alignment via FFT...".to_string());
    let correlation_offset = crate::correlation::find_best_global_offset(
        &ref_timespans,
        &ref_energy,
        &tgt_blocks_orig,
        config.global_search_window_ms, 
        &mut progress_callback
    );
    #[cfg(target_arch = "wasm32")]
    yield_now().await;

    progress_callback(format!("MATCH: Found optimal global shift at {}ms", correlation_offset));
    progress_callback("Step 4: Refined Linear Regression...".to_string());
    let mut params = crate::regression::find_global_params(&ref_timespans, &tgt_timespans, duration_ms);
    
    params.offset_ms = correlation_offset + config.professional_bias_ms; 
    progress_callback(format!("FINAL: Anchor set to {}ms (Bias applied).", params.offset_ms));
    params.ratio = 1.0; 
    
    log!("VAD Result: {} spans, Energy signal size: {}", ref_timespans.len(), ref_energy.len());
    log!("Correlation Peak: {}ms, Professional Bias: {}ms, Total Offset: {}ms", correlation_offset, config.professional_bias_ms, params.offset_ms);
    
    progress_callback(format!("Definitive Global Anchor: {}ms", params.offset_ms));

    let tgt_normalized: Vec<TimeSpan> = tgt_timespans.iter()
        .map(|ts| {
            let s = (ts.start.as_i64() as f64 * params.ratio).round() as i64 + params.offset_ms;
            let e = (ts.end.as_i64() as f64 * params.ratio).round() as i64 + params.offset_ms;
            TimeSpan::new(TimePoint::from(s), TimePoint::from(e))
        })
        .collect();

    progress_callback(format!("Scaling to 1ms units... (Ref: {}, Tgt: {})", ref_timespans.len(), tgt_normalized.len()));
    let ref_scaled: Vec<TimeSpan> = ref_timespans.iter()
        .map(|ts| TimeSpan::new(TimePoint::from(ts.start.as_i64()), TimePoint::from(ts.end.as_i64())))
        .collect();
    let tgt_scaled: Vec<TimeSpan> = tgt_normalized.iter()
        .map(|ts| TimeSpan::new(TimePoint::from(ts.start.as_i64()), TimePoint::from(ts.end.as_i64())))
        .collect();

    progress_callback(format!("Aligning {} blocks...", tgt_scaled.len()));
    let (deltas, _score) = align(
        &ref_scaled,
        &tgt_scaled,
        config.alass_split_penalty,
        Some(1.2), // Parity weight from CLI
        standard_scoring,
        NoProgressHandler
    );
    #[cfg(target_arch = "wasm32")]
    yield_now().await;

    progress_callback("Alignment complete. Rendering SRT...".to_string());
    let mut synced_blocks = Vec::new();
    for (i, mut block) in tgt_blocks_orig.into_iter().enumerate() {
        // Precise Shift: Linear Regression Anchor + Non-Linear DP Delta
        let base_start = (block.start_ms as f64 * params.ratio).round() as i64 + params.offset_ms;
        let base_end = (block.end_ms as f64 * params.ratio).round() as i64 + params.offset_ms;
        
        let diff_ms = deltas[i].as_i64();
        block.start_ms = (base_start + diff_ms).max(0);
        block.end_ms = (base_end + diff_ms).max(0);
        
        if i < 5 {
            progress_callback(format!("TRACE: Block {} starts at {}ms", i+1, block.start_ms));
        }
        
        synced_blocks.push(block);
    }

    progress_callback("Preparing output...".to_string());
    let out_content = srt::create_srt_string(&synced_blocks);

    progress_callback("Done!".to_string());
    Ok(out_content)
}

// Keep a compatibility wrapper for native if needed, but we'll focus on run_sync_data
#[allow(dead_code)]
pub async fn run_sync(
    ref_path: &str,
    tgt_path: &str,
    _out_path: &str,
    progress_callback: impl FnMut(String),
) -> Result<String, String> {
    let ref_data: Arc<Vec<u8>> = Arc::new(std::fs::read(ref_path).map_err(|e| e.to_string())?);
    let tgt_data: Arc<Vec<u8>> = Arc::new(std::fs::read(tgt_path).map_err(|e| e.to_string())?);
    run_sync_data(ref_data, ref_path, tgt_data, tgt_path, progress_callback).await
}
