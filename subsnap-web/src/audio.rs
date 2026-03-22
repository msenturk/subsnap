use futures_util::SinkExt;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::probe::Hint;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::errors::Error;
use std::sync::Arc;
use std::io::{Read, Seek, SeekFrom};

// Cross-platform logging/timing utilities
macro_rules! log {
    ($($t:tt)*) => {
        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&format!($($t)*).into());
        #[cfg(not(target_arch = "wasm32"))]
        println!("[LOG] {}", format!($($t)*));
    }
}

#[allow(dead_code)]
pub enum AudioReceiver {
    Native(std::sync::mpsc::Receiver<Result<Vec<f32>, String>>),
    Wasm(futures_channel::mpsc::Receiver<Result<Vec<f32>, String>>),
    #[cfg(target_arch = "wasm32")]
    WasmDirect {
        format: Box<dyn symphonia::core::formats::FormatReader>,
        decoder: Box<dyn symphonia::core::codecs::Decoder>,
        track_id: u32,
        channel_count: usize,
        packet_count: u32,
    },
}

#[allow(dead_code)]
pub struct StreamedAudio {
    pub receiver: AudioReceiver,
    pub sample_rate: u32,
    pub total_samples: Option<u64>,
}

pub struct ArcSource {
    pub data: Arc<Vec<u8>>,
    pub pos: u64,
}

impl ArcSource {
    pub fn new(data: Arc<Vec<u8>>) -> Self {
        Self { data, pos: 0 }
    }
}

impl Read for ArcSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (&self.data[self.pos as usize..]).read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ArcSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(p) => self.data.len() as i64 + p,
            SeekFrom::Current(p) => self.pos as i64 + p,
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "negative seek"));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}

impl MediaSource for ArcSource {
    fn is_seekable(&self) -> bool { true }
    fn byte_len(&self) -> Option<u64> { Some(self.data.len() as u64) }
}

#[allow(dead_code)]
pub fn stream_audio_from_source(source: Box<dyn MediaSource>, extension: Option<&str>) -> Result<StreamedAudio, String> {
    let mss = MediaSourceStream::new(source, Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }

    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .map_err(|e| format!("unsupported format: {}", e))?;

    let format = probed.format;

    let track = format.tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| "no supported audio tracks".to_string())?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let total_samples = track.codec_params.n_frames;
    let codec_params = track.codec_params.clone();

    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<f32>, String>>(); 
        std::thread::spawn(move || {
            decode_loop_native(format, track_id, codec_params, tx);
        });
        Ok(StreamedAudio {
            receiver: AudioReceiver::Native(rx),
            sample_rate,
            total_samples,
        })
    }

    #[cfg(target_arch = "wasm32")]
    {
        let (tx, rx) = futures_channel::mpsc::channel::<Result<Vec<f32>, String>>(32);
        wasm_bindgen_futures::spawn_local(async move {
            decode_loop_wasm(format, track_id, codec_params, tx).await;
        });
        Ok(StreamedAudio {
            receiver: AudioReceiver::Wasm(rx),
            sample_rate,
            total_samples,
        })
    }
}

/// Decode entire audio and run VAD incrementally to save memory.
/// Returns (timespans, energy_envelope, sample_rate).
pub async fn decode_all_to_memory(
    data: Arc<Vec<u8>>, 
    filename: &str,
    config: crate::config::SyncConfig,
    mut progress_cb: impl FnMut(String),
) -> Result<(Vec<crate::vad::TimeSpan>, Vec<f32>, u32), String> {
    let source = Box::new(ArcSource::new(data));
    let mss = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = filename.rsplit('.').next() { hint.with_extension(ext); }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| format!("unsupported format: {}", e))?;

    let mut format = probed.format;
    let track = format.tracks()
        .iter()
        .filter(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .max_by_key(|t| t.codec_params.n_frames.unwrap_or(0))
        .ok_or("no audio tracks")?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let total_samples_est = track.codec_params.n_frames;

    log!("DECODE_ALL: Selected Track ID {} with {:?} expected frames", track_id, total_samples_est);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder error: {}", e))?;

    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut vad = crate::vad::StreamingVad::new(sample_rate, config);
    let mut samples_processed = 0u64;
    let mut last_yield_time = crate::vad::get_now_ms();

    log!("DECODE_STREAM: Infinite-Duration Mode (Streaming VAD). rate={}", sample_rate);

    'decode: loop {
        let now = crate::vad::get_now_ms();
        if now - last_yield_time > 100.0 {
            #[cfg(target_arch = "wasm32")]
            crate::sync::yield_now().await;
            last_yield_time = crate::vad::get_now_ms();

            if samples_processed % 300_000 == 0 {
                if let Some(total) = total_samples_est {
                    let total_dur_min = (samples_processed as f64 / sample_rate as f64 / 60.0).round();
                    let mut progress = samples_processed as f32 / total as f32;
                    progress = progress.min(1.0);
                    progress_cb(format!("PROGRESS_VAD:{}% ({} min decoded)", (progress * 100.0) as i32, total_dur_min));
                } else {
                    // No estimate, just show current minutes
                    let mins = samples_processed / (sample_rate as u64 * 60);
                    progress_cb(format!("LOG:Processing... {} mins", mins));
                }
            }
        }

        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break 'decode,
            Err(Error::ResetRequired) => continue,
            Err(e) => {
                log!("DECODE_STREAM: Error {:?}", e);
                break 'decode;
            }
        };

        if packet.track_id() != track_id { continue; }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = *decoded.spec();
                let ch = spec.channels.count();
                let dur = decoded.frames() as u64;
                if sample_buf.is_none() || sample_buf.as_ref().unwrap().capacity() < decoded.frames() {
                    sample_buf = Some(SampleBuffer::<f32>::new(dur, spec));
                }
                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(decoded);
                    let samples = buf.samples();
                    
                    let mut downmixed = Vec::with_capacity(samples.len() / ch as usize);
                    if ch == 1 {
                        for &s in samples { 
                            downmixed.push(if s.is_finite() { s } else { 0.0 }); 
                        }
                    } else if ch >= 3 {
                        for chunk in samples.chunks_exact(ch as usize) {
                            let avg = (chunk[0] + chunk[1] + chunk[2]) / 3.0;
                            downmixed.push(if avg.is_finite() { avg } else { 0.0 });
                        }
                    } else {
                        for chunk in samples.chunks_exact(ch as usize) {
                            let avg = chunk.iter().sum::<f32>() / ch as f32;
                            downmixed.push(if avg.is_finite() { avg } else { 0.0 });
                        }
                    }
                    
                    if samples_processed < 5000 {
                        let peak = downmixed.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
                        if samples_processed % 1000 == 0 {
                            log!("AUD_HEALTH: Initial Sample Peak: {:.4}", peak);
                        }
                    }
                    vad.process_chunk(&downmixed);
                    samples_processed += downmixed.len() as u64;
                }
            }
            Err(_) => continue,
        }
    }

    let (spans, energy) = vad.finalize();
    Ok((spans, energy, sample_rate))
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
async fn decode_loop_wasm(
    mut format: Box<dyn symphonia::core::formats::FormatReader>,
    track_id: u32,
    codec_params: symphonia::core::codecs::CodecParameters,
    mut tx: futures_channel::mpsc::Sender<Result<Vec<f32>, String>>,
) {
    web_sys::console::log_1(&"DECODER: Task started!".into());

    let dec_opts: DecoderOptions = Default::default();
    let mut decoder = match symphonia::default::get_codecs().make(&codec_params, &dec_opts) {
        Ok(d) => d,
        Err(e) => {
            web_sys::console::log_1(&format!("DECODER: Failed to make decoder: {}", e).into());
            let _ = SinkExt::send(&mut tx, Result::<Vec<f32>, String>::Err(format!("failed to create decoder: {}", e))).await;
            return;
        }
    };

    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut channel_count = 1;
    let mut packet_count = 0u32;
    let mut audio_count = 0u32;

    loop {
        // Use setTimeout-based yield to fully release the JS thread each iteration
        crate::sync::yield_now().await;

        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                web_sys::console::log_1(&format!("DECODER: EOF. packets={} audio={}", packet_count, audio_count).into());
                break;
            }
            Err(Error::ResetRequired) => continue,
            Err(e) => {
                web_sys::console::log_1(&format!("DECODER: Fatal error: {:?}", e).into());
                break;
            }
        };

        packet_count += 1;
        if packet_count <= 3 {
            web_sys::console::log_1(&format!("DECODER: pkt#{} track={} target={}", packet_count, packet.track_id(), track_id).into());
        }

        if packet.track_id() != track_id {
            continue;
        }
        audio_count += 1;

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if audio_count == 1 {
                    web_sys::console::log_1(&"DECODER: First audio packet decoded!".into());
                }
                if sample_buf.is_none() || sample_buf.as_ref().unwrap().capacity() < decoded.frames() {
                    let spec = *decoded.spec();
                    channel_count = spec.channels.count() as usize;
                    let duration = decoded.frames() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(decoded);
                    let samples = buf.samples();
                    let channels = channel_count;
                    let mut mono = Vec::with_capacity(samples.len() / channels);
                    for chunk in samples.chunks_exact(channels) {
                        let sum: f32 = chunk.iter().sum();
                        mono.push(sum / channels as f32);
                    }
                    if SinkExt::send(&mut tx, Result::<Vec<f32>, String>::Ok(mono)).await.is_err() {
                        log!("DECODER: Channel closed, stopping.");
                        break;
                    }
                }
            }
            Err(e) => {
                if audio_count <= 3 {
                    log!("DECODER: Decode error pkt#{}: {:?}", audio_count, e);
                }
                continue;
            }
        }
    }
    log!("DECODER: Done. {} total, {} audio packets.", packet_count, audio_count);
}

#[allow(dead_code)]
fn decode_loop_native(
    mut format: Box<dyn symphonia::core::formats::FormatReader>,
    track_id: u32,
    codec_params: symphonia::core::codecs::CodecParameters,
    tx: std::sync::mpsc::Sender<Result<Vec<f32>, String>>,
) {
    let dec_opts: DecoderOptions = Default::default();
    let mut decoder = match symphonia::default::get_codecs().make(&codec_params, &dec_opts) {
        Ok(d) => d,
        Err(e) => {
            let _ = tx.send(Err(format!("failed to create decoder: {}", e)));
            return;
        }
    };

    let mut sample_buf: Option<SampleBuffer<f32>> = None;
    let mut channel_count = 1;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => {
                let _ = tx.send(Err(format!("next_packet error: {}", e)));
                break;
            }
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                if sample_buf.is_none() || sample_buf.as_ref().unwrap().capacity() < decoded.frames() {
                    let spec = *decoded.spec();
                    channel_count = spec.channels.count() as usize;
                    let duration = decoded.frames() as u64;
                    sample_buf = Some(SampleBuffer::<f32>::new(duration, spec));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(decoded);
                    let samples = buf.samples();
                    let channels = channel_count;
                    
                    let mut mono = Vec::with_capacity(samples.len() / channels);
                    if channels == 1 {
                        mono.extend_from_slice(samples);
                    } else if channels >= 3 {
                        for chunk in samples.chunks_exact(channels) {
                            mono.push(chunk[2]);
                        }
                    } else {
                        for chunk in samples.chunks_exact(channels) {
                            let sum: f32 = chunk.iter().sum();
                            mono.push(sum / channels as f32);
                        }
                    }
                    
                    if tx.send(Ok(mono)).is_err() {
                        break;
                    }
                }
            }
            Err(_) => continue,
        }
    }
}
