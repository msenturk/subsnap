pub use alass_core::{TimePoint, TimeSpan};
#[allow(unused_imports)]
use crate::audio::AudioReceiver;

// Cross-platform logging/timing utilities
macro_rules! log {
    ($($t:tt)*) => {
        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&format!($($t)*).into());
        #[cfg(not(target_arch = "wasm32"))]
        println!($($t)*);
    }
}

pub fn get_now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now();
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as f64;
}

pub struct StreamingVad {
    resampler: StreamedResampler,
    energy_envelope: Vec<f32>,
    raw_timespans: Vec<(i64, i64)>,
    
    voice_start_ms: Option<i64>,
    last_voice_ms: i64,
    frame_count: usize,
    
    frame_duration_ms: i64,
    frame_size: usize,
    target_rate: u32,
    
    running_peak: f32,
    last_sample: f32, // For High-Pass Vocal filter
    config: crate::config::SyncConfig,
}

impl StreamingVad {
    pub fn new(original_rate: u32, config: crate::config::SyncConfig) -> Self {
        let target_rate = 16000u32;
        let frame_duration_ms = 10;
        let frame_size = (target_rate as usize * frame_duration_ms as usize) / 1000;
        Self {
            resampler: StreamedResampler::new(original_rate, target_rate),
            energy_envelope: Vec::with_capacity(360000), 
            raw_timespans: Vec::new(),
            voice_start_ms: None,
            last_voice_ms: 0,
            frame_count: 0,
            frame_duration_ms: frame_duration_ms as i64,
            frame_size,
            target_rate,
            running_peak: 0.1,
            last_sample: 0.0,
            config,
        }
    }

    pub fn process_chunk(&mut self, chunk: &[f32]) {
        let resampled = self.resampler.process_chunk(chunk);
        if resampled.is_empty() { return; }

        for frame in resampled.chunks_exact(self.frame_size) {
            let current_time_ms = self.frame_count as i64 * self.frame_duration_ms;
            
            // Peak Following (for adaptive volume in streaming mode)
            let max_abs = frame.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
            self.running_peak = self.running_peak.max(max_abs);

            // Digital Speech Filter (Pre-emphasis) + RMS
            let mut sum_sq: f32 = 0.0;
            let mut zcr_count = 0;
            let mut prev_vocal = self.last_sample;

            for &s in frame {
                // High-pass filter to isolate human vocal frequencies (~300Hz+)
                let vocal = s - 0.97 * self.last_sample;
                self.last_sample = s; 

                sum_sq += vocal.powi(2);
                if (vocal > 0.0) != (prev_vocal > 0.0) { zcr_count += 1; }
                prev_vocal = vocal;
            }

            let mut rms = (sum_sq / self.frame_size as f32).sqrt();
            if !rms.is_finite() { rms = 0.0; }
            self.energy_envelope.push(rms);

            let zcr = zcr_count as f32 / self.frame_size as f32;

            // VOICING logic (Standard Alass/FFSubSync baseline)
            // 1. RMS > 0.015 (Very sensitive whisper-detect)
            // 2. ZCR 0.05..0.5 (Human frequency range)
            let is_voice = rms > 0.015 && zcr > 0.05 && zcr < 0.5;

            if is_voice {
                if self.voice_start_ms.is_none() { self.voice_start_ms = Some(current_time_ms); }
                self.last_voice_ms = current_time_ms + self.frame_duration_ms;
            } else if let Some(start) = self.voice_start_ms {
                if current_time_ms - self.last_voice_ms > self.config.vad_gap_tolerance_ms {
                    if self.last_voice_ms - start >= self.config.vad_min_voice_ms {
                        self.raw_timespans.push((start, self.last_voice_ms));
                    }
                    self.voice_start_ms = None;
                }
            }
            self.frame_count += 1;
        }
    }

    pub fn finalize(mut self) -> (Vec<TimeSpan>, Vec<f32>) {
        if let Some(start) = self.voice_start_ms {
            self.raw_timespans.push((start, self.last_voice_ms));
        }

        let mut timespans: Vec<TimeSpan> = Vec::new();
        let mut current_merged: Option<(i64, i64)> = None;
        let lead_in_ms = self.config.vad_lead_in_ms;

        for (start, end) in self.raw_timespans {
            let biased_start = (start - lead_in_ms).max(0);
            if let Some((m_start, m_end)) = current_merged {
                if biased_start <= m_end {
                    current_merged = Some((m_start, end.max(m_end)));
                } else {
                    timespans.push(TimeSpan::new(TimePoint::from(m_start), TimePoint::from(m_end)));
                    current_merged = Some((biased_start, end));
                }
            } else {
                current_merged = Some((biased_start, end));
            }
        }
        if let Some((m_start, m_end)) = current_merged {
            timespans.push(TimeSpan::new(TimePoint::from(m_start), TimePoint::from(m_end)));
        }

        log!("VAD_STREAM: Finalized. frames={}, segments={}, max_peak={:.3}", self.frame_count, timespans.len(), self.running_peak);
        (timespans, self.energy_envelope)
    }
}

struct StreamedResampler {
    in_rate: u32,
    out_rate: u32,
    current_in_idx: f64,
    leftover_samples: Vec<f32>,
}

impl StreamedResampler {
    fn new(in_rate: u32, out_rate: u32) -> Self {
        Self { in_rate, out_rate, current_in_idx: 0.0, leftover_samples: Vec::new() }
    }

    fn process_chunk(&mut self, chunk: &[f32]) -> Vec<f32> {
        if self.in_rate == self.out_rate { return chunk.to_vec(); }
        if chunk.is_empty() { return Vec::new(); }

        let ratio = self.in_rate as f64 / self.out_rate as f64;
        let mut out = Vec::new();
        let mut available = Vec::with_capacity(self.leftover_samples.len() + chunk.len());
        available.extend_from_slice(&self.leftover_samples);
        available.extend_from_slice(chunk);
        
        let start_idx_floor = self.current_in_idx.floor();
        loop {
            let local_idx1 = (self.current_in_idx - start_idx_floor) as usize;
            let local_idx2 = local_idx1 + 1;

            if local_idx2 >= available.len() {
                self.leftover_samples = if local_idx1 < available.len() { available[local_idx1..].to_vec() } else { Vec::new() };
                break;
            }

            let weight = (self.current_in_idx - self.current_in_idx.floor()) as f32;
            out.push(available[local_idx1] * (1.0 - weight) + available[local_idx2] * weight);
            self.current_in_idx += ratio;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Unified VAD on pre-decoded samples (keeps signature for sync.rs compat)
// ---------------------------------------------------------------------------
#[allow(dead_code)]
pub async fn run_vad_on_samples(
    samples_i16: &[i16],
    original_rate: u32,
    _total_samples: Option<u64>,
    config: &crate::config::SyncConfig,
    _start_offset_ms: i64,
    _progress_callback: impl FnMut(String),
) -> Result<(Vec<TimeSpan>, Vec<f32>), String> {
    let mut vad = StreamingVad::new(original_rate, config.clone());
    
    // Process in large chunks for speed
    let f32_chunk: Vec<f32> = samples_i16.iter().map(|&x| x as f32 / 32767.0).collect();
    vad.process_chunk(&f32_chunk);
    
    Ok(vad.finalize())
}
