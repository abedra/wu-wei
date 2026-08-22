use std::f32::consts::{PI, TAU};

use eframe::egui;

/// Renders the app mark: an ensō, the single-stroke Zen circle left
/// deliberately open — a fitting emblem for Wu Wei (effortless, unforced
/// action). Drawn procedurally (radial distance field, antialiased) rather
/// than shipped as an asset, so window icon and in-UI logo always match and
/// can be requested at whatever resolution the caller needs.
///
/// Returns unmultiplied-alpha RGBA, `size * size * 4` bytes, transparent
/// outside the stroke.
pub fn enso_rgba(size: u32, color: egui::Color32) -> Vec<u8> {
    let n = size as f32;
    let center = n / 2.0;
    let outer = n * 0.42;
    let base_width = n * 0.12;

    // The brush lifts off the page here, opening the circle; `gap_half` is
    // the fully-open span either side of that point, `taper` the additional
    // arc over which the stroke fades back in.
    let gap_center = -PI / 4.0; // upper-right
    let gap_half = 0.22;
    let taper = 0.32;

    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let d = dx.hypot(dy);
            let theta = dy.atan2(dx);

            let mut delta = (theta - gap_center).rem_euclid(TAU);
            if delta > PI {
                delta = TAU - delta;
            }
            let opening = smoothstep(gap_half, gap_half + taper, delta);
            if opening <= 0.0 {
                continue;
            }

            // Hand-drawn imperfection: the radius and stroke pressure wander
            // a little around the circle instead of tracing a perfect ring.
            let wobble = (theta * 3.0 + 0.6).sin() * outer * 0.035;
            let ring_radius = outer + wobble;
            let pressure = 0.82 + 0.18 * (theta * 1.3 + 1.0).sin();
            let width = base_width * opening * pressure;
            if width < 0.5 {
                continue;
            }

            let half = width / 2.0;
            let inner_edge = ring_radius - half;
            let outer_edge = ring_radius + half;
            let coverage = smoothstep(inner_edge - 1.0, inner_edge, d)
                * (1.0 - smoothstep(outer_edge, outer_edge + 1.0, d));
            if coverage <= 0.0 {
                continue;
            }

            let i = ((y * size + x) * 4) as usize;
            rgba[i] = color.r();
            rgba[i + 1] = color.g();
            rgba[i + 2] = color.b();
            rgba[i + 3] = (coverage * 255.0).round() as u8;
        }
    }
    rgba
}

/// The same mark as [`enso_rgba`], as a window/taskbar icon.
pub fn window_icon(color: egui::Color32) -> egui::IconData {
    const SIZE: u32 = 256;
    egui::IconData {
        rgba: enso_rgba(SIZE, color),
        width: SIZE,
        height: SIZE,
    }
}

/// The same mark as [`enso_rgba`], PNG-encoded — for the icon file a
/// `.desktop` entry points at (see `install-desktop`). Wayland compositors
/// resolve a running app's task-switcher icon through that file, not through
/// the window's own [`IconData`](egui::IconData), so a real file on disk is
/// what actually fixes the "generic gear" icon there.
pub fn enso_png(size: u32, color: egui::Color32) -> Vec<u8> {
    use image::ImageEncoder;

    let rgba = enso_rgba(size, color);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&rgba, size, size, image::ExtendedColorType::Rgba8)
        .expect("encoding a freshly generated RGBA buffer to PNG cannot fail");
    png
}

/// The same mark as [`enso_rgba`], packaged as a macOS `.icns` — for the icon
/// inside an installed `.app` bundle (see `install-desktop` on macOS). Bundles
/// several PNG-encoded sizes (modern ICNS entries carry PNG data directly) so
/// the Dock, Finder and app switcher each have a crisp source to scale from.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn enso_icns(color: egui::Color32) -> Vec<u8> {
    // (OSType, pixel size) pairs, largest last. The `ic0*` types all accept a
    // raw PNG payload, so no legacy ARGB packing is needed.
    const ENTRIES: &[(&[u8; 4], u32)] = &[
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
    ];

    let mut body = Vec::new();
    for (ostype, size) in ENTRIES {
        let png = enso_png(*size, color);
        body.extend_from_slice(*ostype);
        // Each entry's length field counts its own 8-byte header (type + len).
        body.extend_from_slice(&((png.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(&png);
    }

    let mut icns = Vec::with_capacity(body.len() + 8);
    icns.extend_from_slice(b"icns");
    icns.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    icns.extend_from_slice(&body);
    icns
}

/// The same mark as [`enso_rgba`], packaged as a Windows `.ico` — for the
/// icon a Start Menu shortcut points at (see `install-desktop` on Windows).
/// Modern `.ico` entries may carry PNG data directly (supported since Vista),
/// so this reuses [`enso_png`] instead of hand-rolling BMP/DIB encoding.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn enso_ico(color: egui::Color32) -> Vec<u8> {
    const SIZES: &[u32] = &[16, 32, 48, 256];

    let images: Vec<Vec<u8>> = SIZES.iter().map(|&size| enso_png(size, color)).collect();

    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());

    // ICONDIR (6 bytes) + one ICONDIRENTRY (16 bytes) per image, then the
    // image payloads themselves in the same order.
    let mut offset = (6 + 16 * SIZES.len()) as u32;
    for (size, png) in SIZES.iter().zip(&images) {
        let dim = if *size >= 256 { 0u8 } else { *size as u8 }; // 0 means 256
        ico.push(dim);
        ico.push(dim);
        ico.push(0); // color count (not palette-based)
        ico.push(0); // reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &images {
        ico.extend_from_slice(png);
    }
    ico
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
