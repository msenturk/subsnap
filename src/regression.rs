use alass_core::TimeSpan;

pub struct LinearParams {
    pub offset_ms: i64,
    pub ratio: f64,
}

pub fn find_global_params(
    ref_times: &[TimeSpan],
    tgt_times: &[TimeSpan],
    duration_ms: i64,
) -> LinearParams {
    if ref_times.is_empty() || tgt_times.is_empty() {
        return LinearParams { offset_ms: 0, ratio: 1.0 };
    }

    // 1. Create density maps (binary occupancy)
    // We use a 100ms resolution bitset for performance
    let res = 100;
    let map_len = (duration_ms / res as i64) as usize + 1;
    let mut ref_map = vec![false; map_len];
    let mut tgt_map = vec![false; map_len];

    for ts in ref_times {
        let s = (ts.start.as_i64() / res as i64).max(0) as usize;
        let e = (ts.end.as_i64() / res as i64).min(map_len as i64 - 1) as usize;
        for i in s..=e { ref_map[i] = true; }
    }
    for ts in tgt_times {
        let s = (ts.start.as_i64() / res as i64).max(0) as usize;
        let e = (ts.end.as_i64() / res as i64).min(map_len as i64 - 1) as usize;
        for i in s..=e { tgt_map[i] = true; }
    }

    // 2. Sample 20 windows across the timeline
    let num_windows = 20;
    let window_size_map = (30000 / res) as usize; // 30s window
    if map_len <= window_size_map {
        return LinearParams { offset_ms: 0, ratio: 1.0 };
    }
    let step = (map_len - window_size_map) / (num_windows + 1);
    
    let mut points = Vec::new();

    for i in 1..=num_windows {
        let center = i * step;
        let start = (center as i32 - (window_size_map as i32 / 2)).max(0) as usize;
        let _end = start + window_size_map;
        
        // Find best local offset in range [-20s, +20s]
        let mut best_offset_map = 0;
        let mut max_corr = -1;
        
        let search_range = (20000 / res) as i32;
        
        for offset in -search_range..=search_range {
            let mut corr = 0;
            for j in 0..window_size_map {
                let tgt_idx = start + j;
                let ref_idx_i = tgt_idx as i32 + offset;
                if ref_idx_i >= 0 && (ref_idx_i as usize) < map_len && ref_map[ref_idx_i as usize] && tgt_map[tgt_idx] {
                    corr += 1;
                }
            }
            if corr > max_corr {
                max_corr = corr;
                best_offset_map = offset;
            }
        }
        
        // Threshold: only keep points with sufficient activity
        if max_corr > (window_size_map / 20) as i32 {
            points.push((center as f64 * res as f64, best_offset_map as f64 * res as f64));
        }
    }

    if points.len() < 5 {
        return LinearParams { offset_ms: 0, ratio: 1.0 };
    }

    // 3. Robust Mean-Centered Least Squares Fit (y = ax + b)
    // x = time, y = offset_at_time
    let n = points.len() as f64;
    let mean_x = points.iter().map(|p| p.0).sum::<f64>() / n;
    let mean_y = points.iter().map(|p| p.1).sum::<f64>() / n;

    let mut num = 0.0;
    let mut den = 0.0;
    for (x, y) in &points {
        let dx = x - mean_x;
        let dy = y - mean_y;
        num += dx * dy;
        den += dx * dx;
    }

    if den.abs() < 1e-9 {
        return LinearParams { offset_ms: mean_y as i64, ratio: 1.0 };
    }

    let slope = num / den;
    let intercept = mean_y - slope * mean_x;

    // slope is the RATE of drift (e.g. 0.001 ms drift per ms of time)
    // SubtitleTime_new = SubtitleTime_old + offset_at_time
    // offset_at_time = slope * time + intercept
    // SubtitleTime_new = time + slope * time + intercept = (1 + slope) * time + intercept

    LinearParams {
        offset_ms: intercept as i64,
        ratio: 1.0 + slope,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alass_core::{TimePoint, TimeSpan};

    #[test]
    fn test_regression_unique_sequence() {
        let mut ref_times = Vec::new();
        let mut tgt_times = Vec::new();
        // Use large gaps (25s) so they don't interfere with search_range (20s)
        for i in 0..10 {
            let start = i * 25000;
            let end = start + 2000;
            ref_times.push(TimeSpan::new(TimePoint::from(start), TimePoint::from(end)));
            
            let offset = 1000;
            tgt_times.push(TimeSpan::new(TimePoint::from(start + offset), TimePoint::from(end + offset)));
        }

        let params = find_global_params(&ref_times, &tgt_times, 300000); // 300s total
        
        // offset_ms should be -1000, ratio should be 1.0 (drift is 0 here)
        assert!((params.offset_ms + 1000).abs() < 150, "Offset {} too far from -1000", params.offset_ms);
        assert!((params.ratio - 1.0).abs() < 0.01, "Ratio {} too far from 1.0", params.ratio);
    }
}
