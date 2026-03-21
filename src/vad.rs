use std::sync::mpsc::Receiver;
use alass_core::{TimePoint, TimeSpan};
use webrtc_vad::{Vad, VadMode, SampleRate};

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
        
        // available buffer represents global input samples starting from floor(self.current_in_idx)
        let mut available = Vec::with_capacity(self.leftover_samples.len() + chunk.len());
        available.extend_from_slice(&self.leftover_samples);
        available.extend_from_slice(chunk);
        
        let start_idx_floor = self.current_in_idx.floor();
        
        loop {
            let local_idx1 = (self.current_in_idx - start_idx_floor) as usize;
            let local_idx2 = local_idx1 + 1;

            if local_idx2 >= available.len() {
                // Save leftover samples starting from local_idx1
                if local_idx1 < available.len() {
                    self.leftover_samples = available[local_idx1..].to_vec();
                } else {
                    self.leftover_samples = Vec::new();
                }
                break;
            }

            let weight = (self.current_in_idx - self.current_in_idx.floor()) as f32;
            let p1 = available[local_idx1];
            let p2 = available[local_idx2];
            out.push(p1 * (1.0 - weight) + p2 * weight);
            
            self.current_in_idx += ratio;
        }
        out
    }
}

pub fn generate_voice_map_stream(
    receiver: Receiver<Result<Vec<f32>, String>>, 
    original_rate: u32, 
    config: &crate::config::SyncConfig,
    _progress_callback: impl FnMut(String)
) -> Result<(Vec<TimeSpan>, Vec<f32>), String> {
    let target_rate = 16000;
    let mut resampler = StreamedResampler::new(original_rate, target_rate);
    let mut vad = Vad::new_with_rate_and_mode(SampleRate::Rate16kHz, VadMode::Aggressive);
    
    let frame_duration_ms = 10;
    let frame_size = (target_rate as usize * frame_duration_ms) / 1000;
    let lead_in_ms = config.vad_lead_in_ms; 
    let gap_tolerance_ms = config.vad_gap_tolerance_ms; 
    let min_voice_duration_ms = config.vad_min_voice_ms; 

    let mut raw_timespans = Vec::new();
    let mut energy_envelope = Vec::new();
    let mut voice_start_ms: Option<i64> = None;
    let mut last_voice_ms: i64 = 0;
    let mut total_frames_processed = 0;
    
    let mut i16_buffer = Vec::new();

    while let Ok(res) = receiver.recv() {
        let chunk = res.map_err(|e| format!("Audio decode error: {}", e))?;
        let resampled = resampler.process_chunk(&chunk);
        
        for s in resampled {
            let clamped = s.clamp(-1.0, 1.0);
            i16_buffer.push((clamped * 32767.0) as i16);
            
            if i16_buffer.len() >= frame_size {
                let frame = &i16_buffer[..frame_size];
                let current_time_ms = (total_frames_processed * frame_duration_ms) as i64;
                
                // Calculate RMS Energy for Phase 11.2
                let sum_sq: f32 = frame.iter().map(|&x| (x as f32 / 32767.0).powi(2)).sum();
                let rms = (sum_sq / frame_size as f32).sqrt();
                energy_envelope.push(rms);

                let is_voice = vad.is_voice_segment(frame).unwrap_or(false);

                if is_voice {
                    if voice_start_ms.is_none() {
                        voice_start_ms = Some(current_time_ms);
                    }
                    last_voice_ms = current_time_ms + frame_duration_ms as i64;
                } else if let Some(start) = voice_start_ms {
                    if current_time_ms - last_voice_ms > gap_tolerance_ms {
                        let duration = last_voice_ms - start;
                        if duration >= min_voice_duration_ms as i64 {
                            raw_timespans.push((start, last_voice_ms));
                        }
                        voice_start_ms = None;
                    }
                }
                
                i16_buffer.drain(..frame_size);
                total_frames_processed += 1;
                if total_frames_processed % 10000 == 0 {
                    // Muted intentionally to prevent UI flooding
                }
            }
        }
    }
    
    if let Some(start) = voice_start_ms {
        raw_timespans.push((start, last_voice_ms));
    }

    // Merge logic
    let mut timespans: Vec<TimeSpan> = Vec::new();
    let mut current_merged: Option<(i64, i64)> = None;

    for (start, end) in raw_timespans {
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

    Ok((timespans, energy_envelope))
}
