use std::fs;
use alass_core::{TimePoint, TimeSpan};

#[derive(Clone, Debug)]
pub struct SubtitleBlock {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

pub fn parse_srt(path: &str) -> Result<Vec<SubtitleBlock>, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let content = String::from_utf8_lossy(&bytes);
    
    if content.starts_with("WEBVTT") || path.ends_with(".vtt") {
        return parse_vtt(&content);
    }

    let ext = std::path::Path::new(path).extension();
    let format = subparse::get_subtitle_format(ext, &bytes)
        .ok_or_else(|| "Unknown subtitle format".to_string())?;

    let sub_file = subparse::parse_bytes(format, &bytes, None, 23.976)
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

fn parse_vtt(content: &str) -> Result<Vec<SubtitleBlock>, String> {
    let mut blocks = Vec::new();
    let mut current_start = 0;
    let mut current_end = 0;
    let mut current_text = Vec::new();
    let mut in_block = false;

    for line in content.lines() {
        let line = line.trim();
        if line.contains("-->") {
            // Save previous block
            if in_block {
                blocks.push(SubtitleBlock {
                    start_ms: current_start,
                    end_ms: current_end,
                    text: current_text.join("\n"),
                });
                current_text.clear();
            }

            let parts: Vec<&str> = line.split("-->").collect();
            if parts.len() == 2 {
                current_start = parse_vtt_timestamp(parts[0].trim())?;
                current_end = parse_vtt_timestamp(parts[1].trim())?;
                in_block = true;
            }
        } else if in_block {
            if line.is_empty() {
                blocks.push(SubtitleBlock {
                    start_ms: current_start,
                    end_ms: current_end,
                    text: current_text.join("\n"),
                });
                current_text.clear();
                in_block = false;
            } else {
                // Skip numeric IDs if they appear just before timestamps
                if line.chars().all(|c| c.is_ascii_digit()) && current_text.is_empty() {
                    continue;
                }
                current_text.push(line.to_string());
            }
        }
    }

    if in_block {
        blocks.push(SubtitleBlock {
            start_ms: current_start,
            end_ms: current_end,
            text: current_text.join("\n"),
        });
    }

    Ok(blocks)
}

fn parse_vtt_timestamp(s: &str) -> Result<i64, String> {
    // 00:00:18.416 or 00:18.416
    let s = s.replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    
    let (h, m, s_ms) = match parts.len() {
        3 => (parts[0], parts[1], parts[2]),
        2 => ("0", parts[0], parts[1]),
        _ => return Err(format!("Invalid timestamp: {}", s)),
    };

    let h_val: i64 = h.parse().map_err(|_| "Invalid hour")?;
    let m_val: i64 = m.parse().map_err(|_| "Invalid minute")?;
    
    let sec_ms_parts: Vec<&str> = s_ms.split('.').collect();
    let (sec, ms) = match sec_ms_parts.len() {
        2 => (sec_ms_parts[0], sec_ms_parts[1]),
        1 => (sec_ms_parts[0], "0"),
        _ => return Err(format!("Invalid second/ms: {}", s_ms)),
    };

    let s_val: i64 = sec.parse().map_err(|_| "Invalid second")?;
    let mut ms_val: i64 = ms.parse().map_err(|_| "Invalid ms")?;
    
    // Handle cases like .4 or .41 (padding to 3 digits)
    if ms.len() == 1 { ms_val *= 100; }
    else if ms.len() == 2 { ms_val *= 10; }

    Ok(h_val * 3600000 + m_val * 60000 + s_val * 1000 + ms_val)
}

pub fn write_srt(path: &str, blocks: &[SubtitleBlock]) -> Result<(), String> {
    let mut out = String::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 { out.push_str("\n\n"); }
        out.push_str(&format!("{}\n", i + 1));
        out.push_str(&format!("{} --> {}\n", format_time(block.start_ms), format_time(block.end_ms)));
        out.push_str(&block.text);
    }
    out.push('\n');
    fs::write(path, out).map_err(|e| e.to_string())
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
