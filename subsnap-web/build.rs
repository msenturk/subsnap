fn main() {
    // Only run this build script if we are targeting windows
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    #[cfg(windows)]
    run_windows_build();
}

#[cfg(windows)]
fn run_windows_build() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon_jpg_path = std::path::Path::new(&manifest_dir).join("icon.jpg");
    let icon_rgba_path = std::path::Path::new(&manifest_dir).join("icon.rgba");
    let icon_ico_path = std::path::Path::new(&manifest_dir).join("icon.ico");

    if icon_jpg_path.exists() {
        if let Ok(img) = image::open(&icon_jpg_path) {
            let resized = img.resize_exact(64, 64, image::imageops::FilterType::Lanczos3);
            let rgba = resized.to_rgba8();
            
            let _ = std::fs::write(&icon_rgba_path, rgba.as_raw());

            let icon_image = ico::IconImage::from_rgba_data(
                64,
                64,
                rgba.into_raw(),
            );
            
            let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
            let _ = ico::IconDirEntry::encode(&icon_image).map(|entry| icon_dir.add_entry(entry));
            
            if let Ok(file) = std::fs::File::create(&icon_ico_path) {
                let mut writer = std::io::BufWriter::new(file);
                let _ = icon_dir.write(&mut writer);
            }
        }
    }

    let mut res = winres::WindowsResource::new();
    if icon_ico_path.exists() {
        res.set_icon(icon_ico_path.to_str().unwrap());
    }
    let _ = res.compile();
}
