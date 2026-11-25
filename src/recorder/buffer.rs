//! Replay buffer: already-encoded video/audio packets kept in RAM for the last
//! N seconds. Nothing is ever written to disk until a clip is saved.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Timestamps are in 100 ns units since the pipeline started.
pub const TICKS: i64 = 10_000_000;

#[derive(Clone)]
pub struct Packet {
    pub pts: i64,
    pub key: bool,
    pub data: Arc<[u8]>,
}

#[derive(Default)]
struct Inner {
    video: VecDeque<Packet>,
    audio: VecDeque<Packet>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    width: u32,
    height: u32,
    fps: u32,
    keep: i64,
}

pub struct Snapshot {
    pub video: Vec<Packet>,
    pub audio: Vec<Packet>,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Default)]
pub struct ClipBuffer(Mutex<Inner>);

impl ClipBuffer {
    pub fn set_keep_seconds(&self, secs: u32) {
        // A little slack so a full clip always starts on a keyframe.
        self.0.lock().unwrap().keep = (secs as i64 + 2) * TICKS;
    }

    pub fn set_video_format(&self, width: u32, height: u32, fps: u32) {
        let mut g = self.0.lock().unwrap();
        g.video.clear();
        g.sps.clear();
        g.pps.clear();
        g.width = width;
        g.height = height;
        g.fps = fps;
    }

    pub fn clear(&self) {
        let mut g = self.0.lock().unwrap();
        g.video.clear();
        g.audio.clear();
    }

    pub fn clear_video(&self) {
        self.0.lock().unwrap().video.clear();
    }

    pub fn push_video(&self, pts: i64, data: Vec<u8>) {
        let mut key = false;
        let mut sps = None;
        let mut pps = None;
        for nal in nal_units(&data) {
            match nal.first().map(|b| b & 0x1F) {
                Some(5) => key = true,
                Some(7) => sps = Some(nal.to_vec()),
                Some(8) => pps = Some(nal.to_vec()),
                _ => {}
            }
        }
        let mut g = self.0.lock().unwrap();
        if let Some(s) = sps {
            g.sps = s;
        }
        if let Some(p) = pps {
            g.pps = p;
        }
        // A stream must start on a keyframe; drop leading non-key frames.
        if g.video.is_empty() && !key {
            return;
        }
        g.video.push_back(Packet { pts, key, data: data.into() });

        // Trim: drop whole GOPs while the *next* keyframe is still old enough
        // to cover the window on its own.
        let limit = pts - g.keep;
        loop {
            let next_key = g.video.iter().skip(1).position(|p| p.key).map(|i| i + 1);
            match next_key {
                Some(k) if g.video[k].pts <= limit => {
                    g.video.drain(..k);
                }
                _ => break,
            }
        }
        let audio_limit = g.video.front().map(|p| p.pts - TICKS).unwrap_or(limit);
        while g.audio.front().is_some_and(|a| a.pts < audio_limit) {
            g.audio.pop_front();
        }
    }

    pub fn push_audio(&self, pts: i64, data: Vec<u8>) {
        let mut g = self.0.lock().unwrap();
        g.audio.push_back(Packet { pts, key: true, data: data.into() });
        if g.video.is_empty() {
            let limit = pts - g.keep;
            while g.audio.front().is_some_and(|a| a.pts < limit) {
                g.audio.pop_front();
            }
        }
    }

    /// Bytes currently held (for the UI / logs).
    pub fn memory_bytes(&self) -> usize {
        let g = self.0.lock().unwrap();
        g.video.iter().chain(g.audio.iter()).map(|p| p.data.len()).sum()
    }

    /// Copy out (cheaply — packets are reference counted) the last `secs` seconds.
    pub fn snapshot(&self, secs: u32) -> Option<Snapshot> {
        let g = self.0.lock().unwrap();
        let newest = g.video.back()?.pts;
        if g.sps.is_empty() || g.pps.is_empty() {
            return None;
        }
        let limit = newest - secs as i64 * TICKS;
        // Latest keyframe at or before the limit; otherwise the oldest we have.
        let start = g
            .video
            .iter()
            .enumerate()
            .filter(|(_, p)| p.key && p.pts <= limit)
            .map(|(i, _)| i)
            .last()
            .unwrap_or(0);
        let video: Vec<Packet> = g.video.iter().skip(start).cloned().collect();
        let t0 = video.first()?.pts;
        let audio = g.audio.iter().filter(|a| a.pts >= t0).cloned().collect();
        Some(Snapshot {
            video,
            audio,
            sps: g.sps.clone(),
            pps: g.pps.clone(),
            width: g.width,
            height: g.height,
            fps: g.fps,
        })
    }
}

/// Split an Annex-B byte stream into NAL units (without start codes).
pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut start: Option<usize> = None;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && (data[i + 2] == 1 || (data[i + 2] == 0 && i + 3 < data.len() && data[i + 3] == 1)) {
            let sc_len = if data[i + 2] == 1 { 3 } else { 4 };
            if let Some(s) = start {
                let mut end = i;
                while end > s && data[end - 1] == 0 {
                    end -= 1;
                }
                if end > s {
                    out.push(&data[s..end]);
                }
            }
            i += sc_len;
            start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(s) = start {
        if s < data.len() {
            out.push(&data[s..]);
        }
    } else if !data.is_empty() {
        // Not Annex-B; treat as one unit.
        out.push(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_nals() {
        let d = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 1, 0x65, 9, 9];
        let n = nal_units(&d);
        assert_eq!(n, vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 9, 9][..]]);
    }

    #[test]
    fn trims_to_window_on_keyframes() {
        let b = ClipBuffer::default();
        b.set_video_format(64, 64, 10);
        b.set_keep_seconds(3); // keeps 5 s
        let sps = [0, 0, 0, 1, 0x67, 1];
        let pps = [0, 0, 0, 1, 0x68, 1];
        for i in 0..200i64 {
            let mut d = Vec::new();
            if i % 10 == 0 {
                d.extend_from_slice(&sps);
                d.extend_from_slice(&pps);
                d.extend_from_slice(&[0, 0, 0, 1, 0x65, 0]);
            } else {
                d.extend_from_slice(&[0, 0, 0, 1, 0x41, 0]);
            }
            b.push_video(i * TICKS / 10, d);
        }
        let s = b.snapshot(3).unwrap();
        assert!(s.video[0].key);
        let span = s.video.last().unwrap().pts - s.video[0].pts;
        assert!(span >= 3 * TICKS && span < 4 * TICKS + TICKS, "span {span}");
    }
}
