#[derive(Clone, Debug)]
pub struct SyncConfig {
    pub vad_lead_in_ms: i64,
    pub vad_gap_tolerance_ms: i64,
    pub vad_min_voice_ms: i64,
    pub global_search_window_ms: i64,
    pub professional_bias_ms: i64,
    pub alass_split_penalty: f64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            vad_lead_in_ms: 1640,
            vad_gap_tolerance_ms: 200,
            vad_min_voice_ms: 150,
            global_search_window_ms: 1200000,
            professional_bias_ms: -1000,
            alass_split_penalty: 100.0,
        }
    }
}
