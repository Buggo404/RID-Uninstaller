//! Program icons as PNG bytes, for the list UI.

use std::path::Path;

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, ReleaseDC,
};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO, PrivateExtractIconsW};

const SIZE: i32 = 48;

/// `sources` are tried in order, separated by `|`. Each is a registry `DisplayIcon` value
/// (`path` or `path,index`), an image file, or a program's folder.
pub fn png(sources: &str) -> Option<Vec<u8>> {
    let sources: Vec<(&str, i32)> = sources
        .split('|')
        .map(|source| match source.rsplit_once(',') {
            Some((p, i)) if i.trim().parse::<i32>().is_ok() => (p, i.trim().parse().unwrap_or(0)),
            _ => (source, 0),
        })
        .map(|(path, index)| (path.trim().trim_matches('"'), index))
        .filter(|(path, _)| !path.is_empty())
        .collect();
    sources.iter().find_map(|&(path, index)| own(Path::new(path), index)).or_else(|| {
        // A program file with no icon of its own gets the one Explorer shows for such files.
        let program = sources.iter().any(|(p, _)| has_extension(Path::new(p), "exe") && Path::new(p).is_file());
        let shell32 = std::env::var("SystemRoot").ok()?;
        program.then(|| extract_rgba(&format!(r"{shell32}\System32\shell32.dll"), 2)).flatten().and_then(|rgba| encode(&rgba))
    })
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

fn own(path: &Path, index: i32) -> Option<Vec<u8>> {
    if has_extension(path, "png") {
        return std::fs::read(path).ok().or_else(|| std::fs::read(scaled_variant(path)?).ok());
    }
    if path.is_dir() {
        return main_programs(path).iter().find_map(|exe| encode(&extract_rgba(&exe.to_string_lossy(), 0)?));
    }
    encode(&extract_rgba(&path.to_string_lossy(), index)?)
}

/// Store apps name `Logo.png` and ship `Logo.scale-200.png`, `Logo.targetsize-48.png` and the like.
fn scaled_variant(path: &Path) -> Option<std::path::PathBuf> {
    let prefix = format!("{}.", path.file_stem()?.to_string_lossy().to_lowercase());
    std::fs::read_dir(path.parent()?)
        .ok()?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.starts_with(&prefix) && name.ends_with(".png") && !name.contains("contrast")
        })
        // The largest file is the most detailed rendering.
        .max_by_key(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .map(|e| e.path())
}

/// The programs directly in `dir`, largest first, without the installer's own helpers:
/// the best guess at what the program's icon is when nothing names one.
fn main_programs(dir: &Path) -> Vec<std::path::PathBuf> {
    // Entries that claim the Windows folder as their own would pick up some system tool's icon.
    let windows = std::env::var("SystemRoot").unwrap_or_default().to_lowercase();
    if windows.is_empty() || dir.to_string_lossy().to_lowercase().starts_with(&windows) {
        return Vec::new();
    }
    let mut programs: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| has_extension(&e.path(), "exe"))
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            !["unins", "setup", "update", "crash", "helper"].iter().any(|helper| name.contains(helper))
        })
        .filter_map(|e| Some((e.metadata().ok()?.len(), e.path())))
        .collect();
    programs.sort_by_key(|(size, _)| std::cmp::Reverse(*size));
    programs.into_iter().take(4).map(|(_, path)| path).collect()
}

fn encode(rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, SIZE as u32, SIZE as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().ok()?.write_image_data(rgba).ok()?;
    Some(out)
}

fn extract_rgba(path: &str, index: i32) -> Option<Vec<u8>> {
    let mut file = [0u16; 260];
    let wide: Vec<u16> = path.encode_utf16().collect();
    if wide.len() >= file.len() {
        return None;
    }
    file[..wide.len()].copy_from_slice(&wide);

    let mut icons = [HICON::default()];
    // SAFETY: `file` is NUL-terminated and `icons` has room for the one icon requested.
    let got = unsafe { PrivateExtractIconsW(&file, index, SIZE, SIZE, Some(&mut icons), None, 0) };
    if got == 0 || got == u32::MAX || icons[0].is_invalid() {
        return None;
    }
    let icon = icons[0];

    let mut info = ICONINFO::default();
    let mut bgra = vec![0u8; (SIZE * SIZE * 4) as usize];
    let mut mask = bgra.clone();
    // SAFETY: handles come from the calls above and are released before returning;
    // both buffers are exactly SIZE x SIZE 32-bit pixels as described by `bmi`.
    let ok = unsafe {
        let ok = GetIconInfo(icon, &mut info).is_ok() && !info.hbmColor.is_invalid() && {
            let dc = GetDC(None);
            let read = |bitmap, pixels: &mut [u8]| {
                let mut bmi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: SIZE,
                        biHeight: -SIZE, // top-down
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                GetDIBits(dc, bitmap, 0, SIZE as u32, Some(pixels.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS) == SIZE
            };
            // Without a readable mask, treat every pixel as opaque (all-black mask).
            let ok = read(info.hbmColor, &mut bgra);
            if info.hbmMask.is_invalid() || !read(info.hbmMask, &mut mask) {
                mask.fill(0);
            }
            ReleaseDC(None, dc);
            ok
        };
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        let _ = DestroyIcon(icon);
        ok
    };
    if !ok {
        return None;
    }

    // Old-style icons carry no alpha channel (it reads as all zero); their shape is in the
    // mask bitmap instead, white where the icon is transparent. Icons with real alpha are
    // left exactly as they are.
    let no_alpha = bgra.chunks_exact(4).all(|p| p[3] == 0);
    for (p, m) in bgra.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
        p.swap(0, 2);
        if no_alpha {
            p[3] = if m[0] != 0 { 0 } else { 255 };
        }
    }
    Some(bgra)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 16x16 icon in the pre-XP format: 24-bit colour, no alpha, shape given by the mask.
    /// All pixels are red; the mask makes the left half transparent.
    fn old_style_ico() -> Vec<u8> {
        let mut image = Vec::new();
        image.extend(40u32.to_le_bytes()); // BITMAPINFOHEADER
        image.extend(16i32.to_le_bytes());
        image.extend(32i32.to_le_bytes()); // colour rows + mask rows
        image.extend(1u16.to_le_bytes());
        image.extend(24u16.to_le_bytes());
        image.extend([0u8; 24]);
        image.extend([0u8, 0, 255].repeat(16 * 16)); // BGR
        image.extend([0xFFu8, 0x00, 0, 0].repeat(16)); // mask: 1 = transparent
        let mut ico = vec![0, 0, 1, 0, 1, 0, 16, 16, 0, 0, 1, 0, 24, 0];
        ico.extend((image.len() as u32).to_le_bytes());
        ico.extend(22u32.to_le_bytes());
        ico.extend(image);
        ico
    }

    #[test]
    fn icons_without_alpha_take_their_shape_from_the_mask() {
        let path = std::env::temp_dir().join(format!("rid-icon-test-{}.ico", std::process::id()));
        std::fs::write(&path, old_style_ico()).unwrap();
        let rgba = extract_rgba(&path.to_string_lossy(), 0);
        std::fs::remove_file(&path).unwrap();
        let rgba = rgba.expect("the icon loads");
        let pixel = |x: usize, y: usize| &rgba[(y * SIZE as usize + x) * 4..][..4];
        assert_eq!(pixel(4, 24)[3], 0, "masked-out half is transparent, not a black square");
        assert_eq!(pixel(44, 24), [255, 0, 0, 255], "visible half keeps its colour");
        assert!(png(r"C:\no\such\icon.exe").is_none());
    }

    #[test]
    fn later_sources_and_scaled_images_stand_in_for_a_missing_icon() {
        let dir = std::env::temp_dir().join(format!("rid-icon-sources-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Logo.scale-100.png"), b"small").unwrap();
        std::fs::write(dir.join("Logo.scale-200.png"), b"the larger one").unwrap();
        std::fs::write(dir.join("Logo.contrast-black.png"), b"larger still, but for high contrast").unwrap();
        let logo = dir.join("Logo.png").to_string_lossy().into_owned();
        assert_eq!(png(&logo).as_deref(), Some(&b"the larger one"[..]));
        assert_eq!(png(&format!(r"NoSuchTool.exe|C:\no\such.ico,3|{logo}")).as_deref(), Some(&b"the larger one"[..]));

        // A folder stands for its main program; one without programs yields nothing.
        assert!(png(&dir.to_string_lossy()).is_none());
        let system32 = format!(r"{}\System32", std::env::var("SystemRoot").unwrap());
        assert!(png(&system32).is_none(), "the Windows folder is nobody's program folder");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
