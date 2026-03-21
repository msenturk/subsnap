use std::fs::File;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::probe::Hint;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::errors::Error;

pub struct StreamedAudio {
    pub receiver: Receiver<Result<Vec<f32>, String>>,
    pub sample_rate: u32,
}

pub fn stream_audio(path_str: String) -> Result<StreamedAudio, String> {
    let path = Path::new(&path_str);
    let src = File::open(&path).map_err(|e| format!("failed to open media: {}", e))?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .map_err(|e| format!("unsupported format: {}", e))?;

    let mut format = probed.format;

    let track = format.tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some())
        .ok_or_else(|| "no supported audio tracks".to_string())?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let codec_params = track.codec_params.clone(); // Clone for the thread

    let (tx, rx) = mpsc::sync_channel::<Result<Vec<f32>, String>>(4096); 

    thread::spawn(move || {
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

        // Optimized Buffer Reuse
        let mut mono_samples = Vec::with_capacity(4096);

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
                        
                        mono_samples.clear();
                        if channels == 1 {
                            mono_samples.extend_from_slice(samples);
                        } else if channels >= 3 {
                            // Extract Center Channel (usually index 2 in 5.1/7.1)
                            for chunk in samples.chunks_exact(channels) {
                                mono_samples.push(chunk[2]);
                            }
                        } else {
                            // Stereo downmix
                            for chunk in samples.chunks_exact(channels) {
                                let sum: f32 = chunk.iter().sum();
                                mono_samples.push(sum / channels as f32);
                            }
                        }
                        
                        if tx.send(Ok(mono_samples.clone())).is_err() {
                            break; // Consumer dropped
                        }
                    }
                }
                Err(Error::IoError(_)) | Err(Error::DecodeError(_)) => continue,
                Err(e) => {
                    let _ = tx.send(Err(format!("decode error: {}", e)));
                    break;
                }
            }
        }
    });

    Ok(StreamedAudio {
        receiver: rx,
        sample_rate,
    })
}
