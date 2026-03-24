use crate::{audio, srt, vad};
use alass_core::{align, standard_scoring, NoProgressHandler, TimeSpan, TimePoint};

use std::sync::Arc;

pub fn run_sync(
    ref_path: &str,
    tgt_path: &str,
    out_path: &str,
    progress_callback: Arc<dyn Fn(String) + Send + Sync + 'static>,
) -> Result<(), String> {
    let config = crate::config::SyncConfig::default();
    progress_callback(format!("Starting sync..."));

    let lower_path = ref_path.to_lowercase();
    let is_media = lower_path.ends_with(".mp4") || lower_path.ends_with(".wav") || lower_path.ends_with(".mkv") || lower_path.ends_with(".avi");
    
    let (ref_timespans, ref_energy) = if is_media {
        progress_callback(format!("Streaming audio from {}...", ref_path));
        let streamed = audio::stream_audio(ref_path.to_string(), progress_callback.clone()).map_err(|e| format!("Audio streaming failed: {}", e))?;

        progress_callback("Running Streamed Voice Activity Detection...".to_string());
        let pc_clone = progress_callback.clone();
        vad::generate_voice_map_stream(streamed.receiver, streamed.sample_rate, &config, move |msg| (pc_clone)(msg))
            .map_err(|e| format!("VAD failed: {}", e))?
    } else {
        progress_callback(format!("Parsing reference SRT: {}", ref_path));
        let ref_blocks = srt::parse_srt(ref_path)?;
        (srt::blocks_to_timespans(&ref_blocks), Vec::new())
    };
 
    if ref_timespans.len() > 5 {
        progress_callback(format!("First 5 Ref VAD segments: {:?}", &ref_timespans[0..5]));
    }

    progress_callback("Parsing target SRT...".to_string());
    let tgt_blocks_orig = srt::parse_srt(tgt_path)?;
    let tgt_timespans = srt::blocks_to_timespans(&tgt_blocks_orig);

    let duration_ms = ref_timespans.last().map(|ts| ts.end.as_i64()).unwrap_or(0);
    
    // Phase 11: FFsubsync-Style Global Correlation
    let pc_clone = progress_callback.clone();
    let correlation_offset = crate::correlation::find_best_global_offset(
        &ref_timespans,
        &ref_energy,
        &tgt_blocks_orig,
        config.global_search_window_ms, 
        &mut move |msg| (pc_clone)(msg)
    );

    progress_callback("Running Global Linear Regression Pre-pass...".to_string());
    let mut params = crate::regression::find_global_params(&ref_timespans, &tgt_timespans, duration_ms);
    
    // Use Correlation as the Definitive Anchor
    params.offset_ms = correlation_offset + config.professional_bias_ms; 
    params.ratio = 1.0; // Force 1:1 for stability unless user asks otherwise
    
    progress_callback(format!("Definitive Global Anchor: {}ms", params.offset_ms));

    // Normalize target timespans for alignment
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

    progress_callback(format!("Aligning {} blocks with split penalty {} (1ms resolution)...", tgt_scaled.len(), config.alass_split_penalty));
    let (_deltas_scaled, _score) = align(
        &ref_scaled,
        &tgt_scaled,
        config.alass_split_penalty,
        Some(2.0),
        standard_scoring,
        NoProgressHandler
    );
    progress_callback("Alignment complete!".to_string());

    // Apply deltas to the ALREADY NORMALIZED blocks
    progress_callback("Applying deltas...".to_string());
    let mut synced_blocks = Vec::new();
    for (_i, mut block) in tgt_blocks_orig.into_iter().enumerate() {
        // First Apply Linear Params (Regression)
        block.start_ms = (block.start_ms as f64 * params.ratio).round() as i64 + params.offset_ms;
        block.end_ms = (block.end_ms as f64 * params.ratio).round() as i64 + params.offset_ms;
        
        // Then Apply Non-linear Delta (Alass) - DISABLED for Pure FFsubsync mode
        // let diff_ms = deltas_scaled[i].as_i64();
        // block.start_ms += diff_ms;
        // block.end_ms += diff_ms;
        synced_blocks.push(block);
    }

    progress_callback(format!("Writing output to {}", out_path));
    srt::write_srt(out_path, &synced_blocks)?;

    progress_callback("Done!".to_string());
    Ok(())
}
