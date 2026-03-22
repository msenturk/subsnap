use std::sync::Arc;
use std::env;

fn main() -> Result<(), String> {
    pollster::block_on(async {
        let args: Vec<String> = env::args().collect();
        if args.len() < 3 { return Err("Usage: debug-sync <media> <srt>".into()); }
        
        let media_path = &args[1];
        let srt_path = &args[2];

        println!("[DEBUG] Universal Synchronizer: Local Testing...");
        
        let media_data = Arc::new(std::fs::read(media_path).map_err(|e| format!("Read error: {}", e))?);
        let srt_data = Arc::new(std::fs::read(srt_path).map_err(|e| format!("Read error: {}", e))?);

        let result = subsnap::sync::run_sync_data(
            media_data,
            media_path,
            srt_data,
            srt_path,
            |msg| println!("[LOG] {}", msg),
        ).await?;

        std::fs::write("debug_output.srt", result).map_err(|e| format!("Write error: {}", e))?;
        println!("[DEBUG] Success! Result written to debug_output.srt");
        Ok(())
    })
}
