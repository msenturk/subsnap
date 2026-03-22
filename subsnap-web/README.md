# SubSnap

A desktop app to align subtitle files (.srt, etc.) with video or other subtitle files.

![Screenshot](screenshot.png)

## How it works

The tool extracts audio from the video and uses Voice Activity Detection (VAD) to find when people are talking. It then uses FFT-based alignment to match the timings in your subtitle file to the actual audio.

- Supports video files (MP4, MKV, AVI, etc.)
- Supports subtitle-to-subtitle synchronization
- Fast multi-threaded processing
- Simple drag-and-drop interface

## Usage

1. **Select Reference**: Drop a video to fix subtitle using audio into the first box.
2. **Select Target**: Drop the subtitle file you want to fix into the second box.
3. **Synchronize**: Click "Start Synchronization". The fixed file will be saved in the same folder as the original.

## Web Version (WASM)

SubSnap now supports running in the browser using WebAssembly. Note: Some features like multi-threading (Rayon) are disabled in the simple web build to ensure compatibility.

Requires [Trunk](https://trunkrs.dev/):
1.  Install Trunk: `cargo install --locked trunk`
2.  Add WASM target: `rustup target add wasm32-unknown-unknown`
3.  Run locally: `trunk serve`
4.  Build for production: `trunk build --release`

## Credits

- `alass-core` for the alignment engine
- `eframe` for the GUI
- `symphonia` for audio decoding
- `webrtc-vad` for voice activity detection (Native only)

## License

MIT
