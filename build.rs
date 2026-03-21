use std::fs::File;
use std::io::BufWriter;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon_jpg_path = std::path::Path::new(&manifest_dir).join("icon.jpg");
    let icon_rgba_path = std::path::Path::new(&manifest_dir).join("icon.rgba");
    let icon_ico_path = std::path::Path::new(&manifest_dir).join("icon.ico");

    // Pre-generate raw RGBA pixels and ICO file to avoid runtime 'image' dependency
    if icon_jpg_path.exists() {
        let img = image::open(&icon_jpg_path).expect("Failed to open icon.jpg");
        // Explicitly resize to 64x64 to minimize binary footprint
        let resized = img.resize_exact(64, 64, image::imageops::FilterType::Lanczos3);
        let rgba = resized.to_rgba8();
        
        // Write raw pixels for main.rs (always overwrite to ensure size is correct)
        std::fs::write(&icon_rgba_path, rgba.as_raw()).expect("Failed to write icon.rgba");

        // Write ICO for Windows binary resource
        if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
            let icon_image = ico::IconImage::from_rgba_data(
                64,
                64,
                rgba.into_raw(),
            );
            let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
            let entry = ico::IconDirEntry::encode(&icon_image).expect("Failed to encode icon entry");
            icon_dir.add_entry(entry);
            
            let file = File::create(&icon_ico_path).expect("Failed to create icon.ico");
            let mut writer = BufWriter::new(file);
            icon_dir.write(&mut writer).expect("Failed to write icon.ico");
        }
    }

    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let mut res = winres::WindowsResource::new();
        if icon_ico_path.exists() {
            res.set_icon(icon_ico_path.to_str().unwrap());
        }
        res.compile().unwrap();
    }
}
