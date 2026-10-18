//! Иконка и сведения о программе в exe (только Windows). Если компилятор ресурсов недоступен,
//! сборка продолжается без иконки.

#[allow(dead_code)]
mod icon {
    include!("src/icon_px.rs");
}

fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| {
            let rgba = icon::icon_rgba(s);
            let mask_row = s.div_ceil(32) * 4;
            let mut out = Vec::new();
            for v in [40u32, s, s * 2] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            for v in [0u32, s * s * 4 + mask_row * s, 0, 0, 0, 0] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            for y in (0..s).rev() {
                for x in 0..s {
                    let i = ((y * s + x) * 4) as usize;
                    out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
                }
            }
            out.extend(std::iter::repeat_n(0u8, (mask_row * s) as usize));
            out
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
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
    println!("cargo:rerun-if-changed=src/icon_px.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("app.ico");
    std::fs::write(&out, ico(&[16, 24, 32, 48, 64, 128, 256])).unwrap();
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(out.to_str().unwrap());
        res.set("ProductName", "SS14 MIDI Studio");
        res.set("FileDescription", "SS14 MIDI Studio — MIDI optimizer and editor for Space Station 14");
        res.set("LegalCopyright", "Berber0s");
        if let Err(e) = res.compile() {
            println!("cargo:warning=иконка exe не встроена: {e}");
        }
    }
}
