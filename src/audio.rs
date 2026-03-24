use std::fs::{self, File};
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

use std::sync::Arc;

pub struct StreamedAudio {
    pub receiver: Receiver<Result<Vec<f32>, String>>,
    pub sample_rate: u32,
}

pub fn stream_audio(path_str: String, log_callback: Arc<dyn Fn(String) + Send + Sync + 'static>) -> Result<StreamedAudio, String> {
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

    for (i, t) in format.tracks().iter().enumerate() {
        let codec = t.codec_params.codec;
        let is_supported = codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some();
        eprintln!("Track {}: codec={:?}, sample_rate={:?}, supported={}",
            i, codec, t.codec_params.sample_rate, is_supported);

        if codec == CODEC_TYPE_NULL {
             eprintln!("  -> Note: Track {} has an unknown or unsupported codec (possibly E-AC3/DDP or DTS).", i);
        }
    }

    let track_res = format.tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL && t.codec_params.sample_rate.is_some());

    if let Some(track) = track_res {
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
    } else {
        // Fallback to ffmpeg
        log_callback("No native tracks found by Symphonia. Attempting FFmpeg fallback...".to_string());
        stream_audio_ffmpeg(path_str, log_callback)
    }
}

fn stream_audio_ffmpeg(path: String, log_callback: Arc<dyn Fn(String) + Send + Sync + 'static>) -> Result<StreamedAudio, String> {
    use std::process::{Command, Stdio};
    use std::io::Read;
    use ffmpeg_sidecar::paths::ffmpeg_path;

    let sample_rate = 16000;
    let args = [
        "-v", "error",
        "-i", &path,
        "-ac", "1",
        "-ar", &sample_rate.to_string(),
        "-f", "f32le",
        "-"
    ];

    // Priority 1: System ffmpeg or common paths
    let mut child_res = Command::new("ffmpeg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    // Priority 1.5: Explicit Scoop check (for Windows users)
    if child_res.is_err() {
        if let Ok(user_profile) = std::env::var("USERPROFILE") {
            let scoop_path = std::path::Path::new(&user_profile).join("scoop").join("shims").join("ffmpeg.exe");
            if scoop_path.exists() {
                child_res = Command::new(scoop_path.as_os_str())
                    .args(&args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
            }
        }
    }

    // Priority 2: ffmpeg-sidecar path (maybe already downloaded)
    if child_res.is_err() {
        let path = ffmpeg_path();
        if path.exists() {
            child_res = Command::new(path.as_os_str())
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
        }
    }

    // Priority 3: Try Package Managers (Universal 'AutoSubSync' style)
    if child_res.is_err() {
        log_callback("FFmpeg not found. Attempting automatic installation via Package Manager...".to_string());
        
        let install_res = if cfg!(target_os = "windows") {
            Command::new("powershell")
                .args(["-Command", "winget install ffmpeg --silent --accept-package-agreements --accept-source-agreements"])
                .status()
        } else if cfg!(target_os = "macos") {
            log_callback("Attempting install via Homebrew...".to_string());
            Command::new("brew").arg("install").arg("ffmpeg").status()
        } else {
            // Check Linux distros
            let mut res = Command::new("sudo").args(["apt", "install", "-y", "ffmpeg"]).status();
            if res.is_ok() && res.as_ref().unwrap().success() {
                log_callback("Attempting install via APT...".to_string());
            } else {
                log_callback("APT failed or not found. Attempting install via DNF...".to_string());
                res = Command::new("sudo").args(["dnf", "install", "-y", "ffmpeg"]).status();
            }
            if res.is_err() || !res.as_ref().unwrap().success() {
                log_callback("DNF failed or not found. Attempting install via Pacman...".to_string());
                res = Command::new("sudo").args(["pacman", "-S", "--noconfirm", "ffmpeg"]).status();
            }
            res
        };

        if let Ok(status) = install_res {
            if status.success() {
                log_callback("Package Manager installation successful. Syncing...".to_string());
                
                // 1. Try PowerShell magic first (Get-Command)
                let ps_find = Command::new("powershell")
                    .args(["-NoProfile", "-Command", "(Get-Command ffmpeg).Source"])
                    .output();

                let mut found_path = None;
                if let Ok(out) = ps_find {
                    let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !path_str.is_empty() {
                        let p = Path::new(&path_str);
                        if p.exists() { found_path = Some(p.to_path_buf()); }
                    }
                }

                // 2. Fallback to aggressive directory scan (Winget/Links common spots)
                if found_path.is_none() && cfg!(target_os = "windows") {
                    let user_profile = std::env::var("USERPROFILE").unwrap_or_default();
                    let search_paths = [
                        format!("{}\\AppData\\Local\\Microsoft\\WinGet\\Links\\ffmpeg.exe", user_profile),
                        "C:\\Program Files\\ffmpeg\\bin\\ffmpeg.exe".to_string(),
                        "C:\\ProgramData\\chocolatey\\bin\\ffmpeg.exe".to_string(),
                        format!("{}\\scoop\\shims\\ffmpeg.exe", user_profile),
                    ];
                    for p_str in search_paths {
                        let p = Path::new(&p_str);
                        if p.exists() { found_path = Some(p.to_path_buf()); break; }
                    }
                }

                let cmd_name = found_path
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "ffmpeg".to_string());
                
                log_callback(format!("Running with detected FFmpeg: {}", cmd_name));

                child_res = Command::new(&cmd_name)
                    .args(&args)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn();
            }
        }
    }

    // Priority 4: Smart Download (If Package Manager failed or was missing)
    if child_res.is_err() {
        log_callback("FFmpeg still not found on system. Downloading standalone 'Essentials' build...".to_string());
        
        let cb = &log_callback;
        cb("Note: If the download stays at 0%, manually download FFmpeg and place the EXE here.".to_string());
        
        // Manual override for essentials build (35MB vs 103MB)
        let essentials_url = if cfg!(target_os = "windows") {
            "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip"
        } else if cfg!(target_os = "linux") {
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-linux64-gpl.tar.xz"
        } else {
            "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip"
        };
        cb(format!("URL: {}", essentials_url));

        use ffmpeg_sidecar::download::unpack_ffmpeg;
        use std::io::Write;

        let temp_archive = Path::new("ffmpeg_download.zip");
        let bin_path = ffmpeg_path();

        // Delete if exists (clean start)
        let _ = fs::remove_file(&temp_archive);

        // Proved ureq download loop
        let resp = ureq::get(essentials_url)
            .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) SubSnap/1.0")
            .call()
            .map_err(|e| format!("Request failed: {}. Visit the URL above to download manually.", e))?;

        let total_size = resp.header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let mut reader = resp.into_reader();
        let mut out = fs::File::create(&temp_archive)
            .map_err(|e| format!("Permission Error: Could not create temp zip in this folder. {}", e))?;
        
        let mut downloaded = 0u64;
        let mut buffer = [0; 65536];
        let mut chunk_count = 0;

        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 { break; }
            out.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            downloaded += n as u64;
            chunk_count += 1;
            
            if chunk_count % 16 == 0 || downloaded == total_size {
                let pct = if total_size > 0 { downloaded * 100 / total_size } else { 0 };
                cb(format!("Downloading Standing FFmpeg: {}% ({}MB / {}MB)...", pct, downloaded / 1024 / 1024, total_size / 1024 / 1024));
            }
        }
        let archive_path = temp_archive.to_path_buf();

        cb("Download complete. Unpacking Archive (this may take 15s)...".to_string());
        unpack_ffmpeg(&archive_path, &bin_path).map_err(|e| format!("Could not unpack FFmpeg: {}. Please delete any 0KB zip files and try again.", e))?;
        
        // Cleanup zip
        let _ = fs::remove_file(&archive_path);
        cb("FFmpeg installation complete.".to_string());

        let final_path = ffmpeg_path();
        if final_path.exists() {
            log_callback(format!("Successfully initialized FFmpeg from: {}", final_path.display()));
            child_res = Command::new(final_path.as_os_str())
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
        }
    }

    let mut child = child_res.map_err(|e| format!("Could not find or start FFmpeg: {}. Please install FFmpeg (e.g. 'scoop install ffmpeg') or ensure you have an internet connection for auto-download.", e))?;
    log_callback("Audio extraction started via FFmpeg stream...".to_string());

    let (tx, rx) = mpsc::sync_channel::<Result<Vec<f32>, String>>(4096);
    let mut stdout = child.stdout.take().ok_or("Failed to open FFmpeg stdout")?;

    thread::spawn(move || {
        let mut buffer = [0u8; 4096 * 4]; // 4096 floats
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let floats_count = n / 4;
                    if floats_count == 0 { continue; }
                    let mut samples = Vec::with_capacity(floats_count);
                    for i in 0..floats_count {
                        let bytes = [buffer[i*4], buffer[i*4+1], buffer[i*4+2], buffer[i*4+3]];
                        samples.push(f32::from_le_bytes(bytes));
                    }
                    if tx.send(Ok(samples)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("FFmpeg read error: {}", e)));
                    break;
                }
            }
        }
        let _ = child.wait();
    });

    Ok(StreamedAudio {
        receiver: rx,
        sample_rate,
    })
}
