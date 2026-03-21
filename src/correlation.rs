use rustfft::{FftPlanner, num_complex::Complex};
use alass_core::TimeSpan;
use crate::srt::SubtitleBlock;


pub fn find_best_global_offset(
    ref_timespans: &[TimeSpan],
    ref_energy: &[f32],
    tgt_subtitles: &[SubtitleBlock],
    max_offset_ms: i64,
    progress_callback: &mut impl FnMut(String),
) -> i64 {
    progress_callback("Generating hybrid binary+energy signals (10ms windows)...".to_string());
    
    let resolution_ms = 10;
    
    // Determine total duration
    let mut total_duration_ms = 0;
    for ts in ref_timespans {
        if ts.end.as_i64() > total_duration_ms {
            total_duration_ms = ts.end.as_i64();
        }
    }
    for sub in tgt_subtitles {
        if sub.end_ms > total_duration_ms {
            total_duration_ms = sub.end_ms;
        }
    }
    
    // Add buffer for shifts
    let total_len = ((total_duration_ms / resolution_ms) + 1) as usize;
    
    // Normalize energy signal
    let max_energy = ref_energy.iter().fold(0.0f32, |m, &x| m.max(x));
    let norm_energy: Vec<f32> = if max_energy > 0.0 {
        ref_energy.iter().map(|&x| x / max_energy).collect()
    } else {
        vec![0.0; ref_energy.len()]
    };

    // Generate signals
    let mut ref_signal = vec![0.0f32; total_len];
    for ts in ref_timespans {
        let start_idx = (ts.start.as_i64() / resolution_ms) as usize;
        let end_idx = (ts.end.as_i64() / resolution_ms) as usize;
        for i in start_idx..end_idx.min(total_len) {
            ref_signal[i] = 1.0;
        }
    }
    
    // Mix Energy into Ref Signal (VAD 0.7 + Energy 0.3)
    for i in 0..total_len.min(norm_energy.len()) {
        ref_signal[i] = (ref_signal[i] * 0.7) + (norm_energy[i] * 0.3);
    }
    
    let mut tgt_signal = vec![0.0f32; total_len];
    for sub in tgt_subtitles {
        let start_idx = (sub.start_ms / resolution_ms) as usize;
        let end_idx = (sub.end_ms / resolution_ms) as usize;
        for i in start_idx..end_idx.min(total_len) {
            tgt_signal[i] = 1.0;
        }
    }

    // Mean-center the signals (Standard Cross-Correlation practice)
    let ref_mean = ref_signal.iter().sum::<f32>() / total_len as f32;
    let tgt_mean = tgt_signal.iter().sum::<f32>() / total_len as f32;
    for x in &mut ref_signal { *x -= ref_mean; }
    for x in &mut tgt_signal { *x -= tgt_mean; }

    // Next power of 2 for FFT
    let fft_len = (total_len * 2).next_power_of_two();
    progress_callback(format!("Performing FFT Cross-Correlation (Size: {})...", fft_len));
    
    let mut ref_complex: Vec<Complex<f32>> = ref_signal.iter().map(|&x| Complex::new(x, 0.0)).collect();
    ref_complex.resize(fft_len, Complex::new(0.0, 0.0));
    
    let mut tgt_complex: Vec<Complex<f32>> = tgt_signal.iter().map(|&x| Complex::new(x, 0.0)).collect();
    tgt_complex.resize(fft_len, Complex::new(0.0, 0.0));
    
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(fft_len);
    
    fft.process(&mut ref_complex);
    fft.process(&mut tgt_complex);
    
    // Cross-correlation in frequency domain: FFT(ref) * Conj(FFT(tgt))
    let mut corr_freq: Vec<Complex<f32>> = ref_complex.iter().zip(tgt_complex.iter()).map(|(r, t)| {
        r * t.conj()
    }).collect();
    
    let ifft = planner.plan_fft_inverse(fft_len);
    ifft.process(&mut corr_freq);
    
    // Find peak within max_offset_ms
    let max_idx_search = (max_offset_ms / resolution_ms) as usize;
    let mut best_score = -1.0;
    let mut best_idx = 0;
    
    // Positive shifts (tgt is late relative to ref)
    for i in 0..max_idx_search.min(fft_len) {
        let score = corr_freq[i].re;
        if score > best_score {
            best_score = score;
            best_idx = i as i64;
        }
    }
    
    // Negative shifts (tgt is early relative to ref)
    // Indexes [fft_len - max_idx_search .. fft_len]
    for i in (fft_len - max_idx_search).max(0)..fft_len {
        let score = corr_freq[i].re;
        if score > best_score {
            best_score = score;
            best_idx = (i as i64) - (fft_len as i64);
        }
    }
    
    let offset_ms = best_idx * resolution_ms;
    progress_callback(format!("Correlation Peak: {}ms (Score: {:.2})", offset_ms, best_score));
    
    offset_ms
}
