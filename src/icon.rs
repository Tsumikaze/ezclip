//! The app icon, drawn procedurally: white ring with a red "record" dot on black.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let c = s / 2.0;
    let corner = s * 0.22;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // rounded-square background coverage
            let qx = (fx - c).abs() - (c - corner);
            let qy = (fy - c).abs() - (c - corner);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - corner;
            let bg = (0.5 - outside).clamp(0.0, 1.0);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt() / s;
            let ring = (1.0 - ((d - 0.33).abs() - 0.045) * s).clamp(0.0, 1.0);
            let dot = (1.0 - (d - 0.19) * s).clamp(0.0, 1.0);
            let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);
            r = r * (1.0 - ring) + 1.0 * ring;
            g = g * (1.0 - ring) + 1.0 * ring;
            b = b * (1.0 - ring) + 1.0 * ring;
            r = r * (1.0 - dot) + 1.0 * dot;
            g = g * (1.0 - dot) + 0.23 * dot;
            b = b * (1.0 - dot) + 0.27 * dot;
            let i = ((y * size + x) * 4) as usize;
            px[i] = (r * 255.0) as u8;
            px[i + 1] = (g * 255.0) as u8;
            px[i + 2] = (b * 255.0) as u8;
            px[i + 3] = (bg * 255.0) as u8;
        }
    }
    px
}
