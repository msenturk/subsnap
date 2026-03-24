use subparse::SubtitleFormat;

fn main() {
    println!("Supported formats:");
    // SubtitleFormat is an enum, so I'll check its variants if possible.
    // Actually, I'll just try to parse a VTT string.
    let formats = [
        SubtitleFormat::SubRip,
        SubtitleFormat::SubStationAlpha,
        SubtitleFormat::AdvancedSubStationAlpha,
    ];
    for f in formats {
        println!("{:?}", f);
    }
}
