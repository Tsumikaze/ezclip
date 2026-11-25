//! The app icon, drawn procedurally: white ring with a coloured dot on black.
//! The dot shows the recorder's state in the tray.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    /// Replay buffer running (red "record" dot).
    Recording,
    /// Starting up (grey dot, dimmed ring).
    Starting,
    /// Something is wrong (amber dot).
    Problem,
}

pub fn rgba(size: u32) -> Vec<u8> {
    rgba_state(size, State::Recording)
}

pub fn rgba_state(size: u32, state: State) -> Vec<u8> {
    let (dot_rgb, ring_level) = match state {
        State::Recording => ([1.0, 0.23, 0.27], 1.0),
        State::Starting => ([0.45, 0.45, 0.45], 0.55),
        State::Problem => ([1.0, 0.72, 0.25], 1.0),
    };
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let c = s / 2.0;
    let corner = s * 0.22;
    // Small tray icons need thicker strokes to stay readable.
    let ring_half = if size <= 24 { 0.06 } else { 0.045 };
    let dot_r = if size <= 24 { 0.21 } else { 0.19 };
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let qx = (fx - c).abs() - (c - corner);
            let qy = (fy - c).abs() - (c - corner);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - corner;
            let bg = (0.5 - outside).clamp(0.0, 1.0);
            let d = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt() / s;
            let ring = (1.0 - ((d - 0.33).abs() - ring_half) * s).clamp(0.0, 1.0) * ring_level;
            let dot = (1.0 - (d - dot_r) * s).clamp(0.0, 1.0);
            let mut col = [ring, ring, ring];
            for k in 0..3 {
                col[k] = col[k] * (1.0 - dot) + dot_rgb[k] * dot;
            }
            let i = ((y * size + x) * 4) as usize;
            px[i] = (col[0] * 255.0) as u8;
            px[i + 1] = (col[1] * 255.0) as u8;
            px[i + 2] = (col[2] * 255.0) as u8;
            px[i + 3] = (bg * 255.0) as u8;
        }
    }
    px
}
