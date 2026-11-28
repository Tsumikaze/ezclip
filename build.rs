//! Embeds the app icon (generated from src/icon.rs, so there is a single
//! source of truth for the logo) and the version info into shear.exe.
#[allow(dead_code)]
#[path = "src/icon.rs"]
mod icon;

/// A .ico file with 32-bit BMP images (alpha channel, empty AND mask).
fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| {
            let rgba = icon::app_icon_rgba(s);
            let mask_row = s.div_ceil(32) * 4;
            let mut b = Vec::new();
            for v in [40u32, s, s * 2] {
                b.extend_from_slice(&v.to_le_bytes()); // biSize, biWidth, biHeight (incl. mask)
            }
            b.extend_from_slice(&1u16.to_le_bytes()); // planes
            b.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
            b.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
            b.extend_from_slice(&(s * s * 4 + mask_row * s).to_le_bytes());
            b.extend_from_slice(&[0u8; 16]);
            for y in (0..s).rev() {
                for x in 0..s {
                    let i = ((y * s + x) * 4) as usize;
                    b.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
                }
            }
            b.resize(b.len() + (mask_row * s) as usize, 0);
            b
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let path = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("shear.ico");
        std::fs::write(&path, ico(&[16, 20, 24, 32, 40, 48, 64, 256])).unwrap();
        let mut res = winresource::WindowsResource::new();
        // Resource id 1: Explorer/taskbar icon.
        res.set_icon_with_id(path.to_str().unwrap(), "1");
        res.set("FileDescription", "Shear");
        res.set("ProductName", "Shear");
        if let Err(e) = res.compile() {
            // Not fatal: the window and tray draw the icon at runtime anyway.
            println!("cargo:warning=could not embed icon: {e}");
        }
    }
}
