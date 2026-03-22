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

    // Atomic Pattern: +1.0 for Speech, -1.0 for Silence (Inherently Mean-Centered)
    let mut ref_signal = vec![-1.0f32; total_len];
    for ts in ref_timespans {
        let start_idx = (ts.start.as_i64() / resolution_ms) as usize;
        let end_idx = (ts.end.as_i64() / resolution_ms) as usize;
        for i in start_idx..end_idx.min(total_len) {
            ref_signal[i] = 1.0;
        }
    }
    
    let mut tgt_signal = vec![-1.0f32; total_len];
    for sub in tgt_subtitles {
        let start_idx = (sub.start_ms / resolution_ms) as usize;
        let end_idx = (sub.end_ms / resolution_ms) as usize;
        for i in start_idx..end_idx.min(total_len) {
            tgt_signal[i] = 1.0;
        }
    }

    // FFT Setup (Next Power of 2)
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
    
    // Identify Candidates (Search for local peaks within the window)
    let mut candidates: Vec<(i64, f32)> = Vec::new();
    
    // Scan all in search window relative to center
    for i in 0..fft_len {
        let lag = if i <= fft_len / 2 { i as i64 } else { (i as i64) - (fft_len as i64) };
        if lag.abs() * resolution_ms <= max_offset_ms {
            candidates.push((lag * resolution_ms, corr_freq[i].re));
        }
    }
    // Rank candidates by Score + Stability (Prefer shifts near 0ms)
    // We apply a tiny penalty (max 10%) for shifts extremely far from 0ms.
    candidates.sort_by(|a, b| {
        let a_weighted = a.1 * (1.0 - (a.0.abs() as f32 / max_offset_ms as f32).min(1.0) * 0.1);
        let b_weighted = b.1 * (1.0 - (b.0.abs() as f32 / max_offset_ms as f32).min(1.0) * 0.1);
        b_weighted.partial_cmp(&a_weighted).unwrap_or(std::cmp::Ordering::Equal)
    });

    if candidates.len() > 3 {
        progress_callback(format!("Top-3 Candidates: [{}ms @ {:.2}, {}ms @ {:.2}, {}ms @ {:.2}]",
            candidates[0].0, candidates[0].1,
            candidates[1].0, candidates[1].1,
            candidates[2].0, candidates[2].1));
    }

    let offset_ms = if !candidates.is_empty() { candidates[0].0 } else { 0 };
    progress_callback(format!("MATCH: Deep-Search Consensus: {}ms (Score: {:.2})", offset_ms, if !candidates.is_empty() { candidates[0].1 } else { 0.0 }));
    
    offset_ms
}
