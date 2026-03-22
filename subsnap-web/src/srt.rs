
use alass_core::{TimePoint, TimeSpan};

#[derive(Clone, Debug)]
pub struct SubtitleBlock {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub fn parse_subtitle_data(bytes: &[u8], filename: &str) -> Result<Vec<SubtitleBlock>, String> {
    let ext = std::path::Path::new(filename).extension();
    let format = subparse::get_subtitle_format(ext, bytes)
        .ok_or_else(|| "Unknown subtitle format".to_string())?;

    let sub_file = subparse::parse_bytes(format, bytes, None, 23.976)
        .map_err(|e| format!("Parsing error: {:?}", e))?;

    let mut blocks = Vec::new();
    let entries = sub_file.get_subtitle_entries().map_err(|e| format!("Entry error: {:?}", e))?;
    
    for entry in entries.iter() {
        blocks.push(SubtitleBlock {
            start_ms: entry.timespan.start.msecs(),
            end_ms: entry.timespan.end.msecs(),
            text: entry.line.clone().unwrap_or_default(),
        });
    }
    Ok(blocks)
}

pub fn create_srt_string(blocks: &[SubtitleBlock]) -> String {
    let mut out = String::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 { out.push_str("\n\n"); }
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!("{} --> {}\n", format_time(block.start_ms), format_time(block.end_ms)));
        out.push_str(&block.text);
    }
    out.push('\n');
    out
}

fn format_time(mut ms: i64) -> String {
    if ms < 0 { ms = 0; }
    let h = ms / 3600000;
    ms %= 3600000;
    let m = ms / 60000;
    ms %= 60000;
    let s = ms / 1000;
    let mili = ms % 1000;
    format!("{:02}:{:02}:{:02},{:03}", h, m, s, mili)
}

pub fn blocks_to_timespans(blocks: &[SubtitleBlock]) -> Vec<TimeSpan> {
    blocks.iter().map(|b| {
        TimeSpan::new(TimePoint::from(b.start_ms), TimePoint::from(b.end_ms))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_time() {
        assert_eq!(format_time(0), "00:00:00,000");
        assert_eq!(format_time(1000), "00:00:01,000");
        assert_eq!(format_time(3600000), "01:00:00,000");
        assert_eq!(format_time(3661001), "01:01:01,001");
        assert_eq!(format_time(-100), "00:00:00,000");
    }

    #[test]
    fn test_blocks_to_timespans() {
        let blocks = vec![
            SubtitleBlock { start_ms: 1000, end_ms: 2000, text: "Hello".to_string() },
        ];
        let timespans = blocks_to_timespans(&blocks);
        assert_eq!(timespans.len(), 1);
        assert_eq!(timespans[0].start.as_i64(), 1000);
        assert_eq!(timespans[0].end.as_i64(), 2000);
    }
}
