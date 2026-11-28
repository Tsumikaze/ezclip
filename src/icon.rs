//! The Shear logo, drawn procedurally: an open pair of classic scissors.
//! * `logo_rgba`: solid white (sidebar logo).
//! * `app_icon_rgba`: white with a black outline on a transparent background
//!   (exe, window, taskbar and tray icon; `build.rs` turns it into the .ico).
//!
//! Shapes are signed distance fields in a unit square (y down), so every size
//! from 16 px to 256 px is drawn crisply with anti-aliased edges.

/// Recorder state, shown in the tray tooltip.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    Recording,
    Starting,
    Problem,
}

type P = (f32, f32);

fn len(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// Distance to a capsule from `a` (radius `ra`) to `b` (radius `rb`).
fn taper(p: P, a: P, b: P, ra: f32, rb: f32) -> f32 {
    let (pax, pay) = (p.0 - a.0, p.1 - a.1);
    let (bax, bay) = (b.0 - a.0, b.1 - a.1);
    let h = ((pax * bax + pay * bay) / (bax * bax + bay * bay)).clamp(0.0, 1.0);
    len(pax - bax * h, pay - bay * h) - (ra + (rb - ra) * h)
}

fn circle(p: P, c: P, r: f32) -> f32 {
    len(p.0 - c.0, p.1 - c.1) - r
}

/// Negative inside the scissors, positive outside (unit-square coordinates).
fn scissors(p: P) -> f32 {
    let pivot = (0.5, 0.47);
    let tip_l = (0.17, 0.05);
    let tip_r = (0.83, 0.05);
    // Each blade continues through the pivot to the handle on the other side.
    let ring_l = (0.285, 0.775);
    let ring_r = (0.715, 0.775);
    let (ring_outer, ring_inner) = (0.165, 0.098);

    let blades = taper(p, pivot, tip_l, 0.072, 0.010).min(taper(p, pivot, tip_r, 0.072, 0.010));
    let shanks = taper(p, pivot, ring_r, 0.05, 0.045).min(taper(p, pivot, ring_l, 0.05, 0.045));
    let rings = circle(p, ring_l, ring_outer).min(circle(p, ring_r, ring_outer));
    let mut d = blades.min(shanks).min(rings);
    // Finger holes and the pivot screw are cut out.
    d = d.max(-circle(p, ring_l, ring_inner)).max(-circle(p, ring_r, ring_inner));
    d.max(-circle(p, pivot, 0.026))
}

/// Coverage of a pixel for a distance in pixels (anti-aliased edge).
fn cover(d_px: f32) -> f32 {
    (0.5 - d_px).clamp(0.0, 1.0)
}

/// Solid white scissors on transparent (RGBA, straight alpha).
pub fn logo_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let d = scissors(((x as f32 + 0.5) / s, (y as f32 + 0.5) / s)) * s;
            let i = ((y * size + x) * 4) as usize;
            px[i..i + 3].copy_from_slice(&[255, 255, 255]);
            px[i + 3] = (cover(d) * 255.0) as u8;
        }
    }
    px
}

/// White scissors with a black outline on transparent (RGBA, straight alpha).
pub fn app_icon_rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    // Outline: ~4.5 % of the icon, at least ~1.2 px so it shows at 16 px.
    let outline = (0.045 * s).max(1.2);
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let d = scissors(((x as f32 + 0.5) / s, (y as f32 + 0.5) / s)) * s;
            let inner = cover(d);
            let outer = cover(d - outline);
            let i = ((y * size + x) * 4) as usize;
            // White where inside, black in the outline band.
            let white = if outer > 0.0 { (inner / outer).min(1.0) } else { 0.0 };
            let v = (white * 255.0) as u8;
            px[i..i + 3].copy_from_slice(&[v, v, v]);
            px[i + 3] = (outer * 255.0) as u8;
        }
    }
    px
}
