//! The "clip saved" sounds: generated in memory (no sound files to ship) and
//! played with PlaySound, so they're heard over a fullscreen game.
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SaveSound {
    Chirp,
    Click,
    Bell,
    Rise,
}

impl SaveSound {
    pub const ALL: [SaveSound; 4] = [Self::Chirp, Self::Click, Self::Bell, Self::Rise];
    pub fn label(self) -> &'static str {
        match self {
            Self::Chirp => "Chirp",
            Self::Click => "Click",
            Self::Bell => "Bell",
            Self::Rise => "Rise",
        }
    }
}

const RATE: u32 = 44_100;

/// Sine burst with a short attack and an exponential (or linear) release.
fn tone(out: &mut Vec<f32>, start_ms: f32, freq: f32, len_ms: f32, decay: f32, gain: f32) {
    let start = (start_ms / 1000.0 * RATE as f32) as usize;
    let n = (len_ms / 1000.0 * RATE as f32) as usize;
    if out.len() < start + n {
        out.resize(start + n, 0.0);
    }
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let attack = (i as f32 / 220.0).min(1.0);
        let release = ((n - i) as f32 / (RATE as f32 * 0.012)).min(1.0); // 12 ms fade-out, no click
        let env = attack * release * if decay > 0.0 { (-t * decay).exp() } else { 1.0 };
        out[start + i] += (t * freq * TAU).sin() * env * gain;
    }
}

fn samples(sound: SaveSound) -> Vec<f32> {
    let mut s = Vec::new();
    match sound {
        SaveSound::Chirp => {
            tone(&mut s, 0.0, 880.0, 75.0, 0.0, 1.0);
            tone(&mut s, 70.0, 1318.5, 95.0, 6.0, 1.0);
        }
        SaveSound::Click => {
            tone(&mut s, 0.0, 1800.0, 30.0, 140.0, 2.0);
            tone(&mut s, 55.0, 2400.0, 30.0, 140.0, 1.8);
        }
        SaveSound::Bell => {
            // Slightly inharmonic partials sound like a small bell.
            for (mult, gain) in [(1.0, 0.7), (2.76, 0.25), (5.4, 0.1)] {
                tone(&mut s, 0.0, 880.0 * mult, 700.0, 5.5, gain);
            }
        }
        SaveSound::Rise => {
            for (i, f) in [1046.5, 1318.5, 1568.0].iter().enumerate() {
                tone(&mut s, i as f32 * 65.0, *f, if i == 2 { 170.0 } else { 62.0 }, 4.0, 1.0);
            }
        }
    }
    s
}

/// A complete WAV file for `sound` at `volume` (0–100).
pub fn wav(sound: SaveSound, volume: u32) -> Vec<u8> {
    // Perceptual curve: 50 ≈ the old fixed level, 100 is loud.
    let amp = 0.95 * (volume.min(100) as f32 / 100.0).powf(1.7);
    let pcm: Vec<i16> = samples(sound).iter().map(|v| ((v * amp).clamp(-1.0, 1.0) * 32767.0) as i16).collect();
    let data_len = (pcm.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// The WAV being played must stay alive while PlaySound reads it.
static PLAYING: Mutex<Vec<u8>> = Mutex::new(Vec::new());

#[cfg(windows)]
pub fn play(sound: SaveSound, volume: u32) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Media::Audio::*;
    if volume == 0 {
        return;
    }
    let mut buf = PLAYING.lock().unwrap();
    unsafe {
        // Stop anything still playing before its buffer is replaced.
        let _ = PlaySoundW(PCWSTR::null(), HMODULE::default(), SND_PURGE);
        *buf = wav(sound, volume);
        let _ = PlaySoundW(PCWSTR(buf.as_ptr() as _), HMODULE::default(), SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}
