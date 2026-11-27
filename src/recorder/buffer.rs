//! Replay buffer: already-encoded video/audio packets kept in RAM for the last
//! N seconds. Nothing is ever written to disk until a clip is saved.
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Timestamps are in 100 ns units since the pipeline started.
pub const TICKS: i64 = 10_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
    Av1,
}

impl VideoCodec {
    /// Parameter sets the container needs, in order (NAL / OBU types).
    fn required_params(self) -> &'static [u8] {
        match self {
            VideoCodec::H264 => &[7, 8],       // SPS, PPS
            VideoCodec::Hevc => &[32, 33, 34], // VPS, SPS, PPS
            VideoCodec::Av1 => &[1],           // sequence header OBU
        }
    }
}

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
    codec: VideoCodec,
    /// Latest parameter sets seen in the stream, by NAL/OBU type.
    params: BTreeMap<u8, Vec<u8>>,
    width: u32,
    height: u32,
    fps: u32,
    keep: i64,
}

pub struct Snapshot {
    pub video: Vec<Packet>,
    pub audio: Vec<Packet>,
    pub codec: VideoCodec,
    /// H.264: [SPS, PPS] · HEVC: [VPS, SPS, PPS] · AV1: [sequence header OBU]
    pub params: Vec<Vec<u8>>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

#[derive(Default)]
pub struct ClipBuffer(Mutex<Inner>);

/// Inspect one encoded frame: is it a keyframe, and which parameter sets does it carry?
fn inspect(codec: VideoCodec, data: &[u8], clean_point: bool) -> (bool, Vec<(u8, Vec<u8>)>) {
    let mut params = Vec::new();
    let mut key = false;
    match codec {
        VideoCodec::H264 => {
            for nal in nal_units(data) {
                match nal[0] & 0x1F {
                    5 => key = true,
                    t @ (7 | 8) => params.push((t, nal.to_vec())),
                    _ => {}
                }
            }
        }
        VideoCodec::Hevc => {
            for nal in nal_units(data) {
                match (nal[0] >> 1) & 0x3F {
                    16..=21 => key = true, // IRAP: BLA / IDR / CRA
                    t @ (32..=34) => params.push((t, nal.to_vec())),
                    _ => {}
                }
            }
        }
        VideoCodec::Av1 => {
            // Keyframes are flagged by the encoder; the sequence header rides along.
            key = clean_point;
            for obu in obus(data) {
                if obu.kind == 1 {
                    params.push((1, obu.whole.to_vec()));
                }
            }
        }
    }
    (key, params)
}

impl ClipBuffer {
    pub fn set_keep_seconds(&self, secs: u32) {
        // A little slack so a full clip always starts on a keyframe.
        self.0.lock().unwrap().keep = (secs as i64 + 2) * TICKS;
    }

    pub fn set_video_format(&self, width: u32, height: u32, fps: u32, codec: VideoCodec) {
        let mut g = self.0.lock().unwrap();
        g.video.clear();
        g.params.clear();
        g.codec = codec;
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

    /// `clean_point`: the encoder marked this frame as a keyframe.
    pub fn push_video(&self, pts: i64, data: Vec<u8>, clean_point: bool) {
        let codec = self.0.lock().unwrap().codec;
        let (key, params) = inspect(codec, &data, clean_point);
        let mut g = self.0.lock().unwrap();
        for (t, p) in params {
            g.params.insert(t, p);
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
        let newest = self.0.lock().unwrap().video.back()?.pts;
        self.snapshot_from(newest - secs as i64 * TICKS)
    }

    /// Everything from the keyframe at or before `limit` (100 ns) up to now.
    pub fn snapshot_from(&self, limit: i64) -> Option<Snapshot> {
        let g = self.0.lock().unwrap();
        if g.video.is_empty() {
            return None;
        }
        let params: Option<Vec<Vec<u8>>> = g.codec.required_params().iter().map(|t| g.params.get(t).cloned()).collect();
        let params = params?;
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
        Some(Snapshot { video, audio, codec: g.codec, params, width: g.width, height: g.height, fps: g.fps })
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
    out.retain(|n| !n.is_empty());
    out
}

/// One AV1 OBU inside a temporal unit.
pub struct Obu<'a> {
    pub kind: u8,
    /// Header + (optional size) + payload, exactly as in the stream.
    pub whole: &'a [u8],
    /// Header bytes (1, or 2 with the extension), without the size field.
    pub header: &'a [u8],
    pub payload: &'a [u8],
}

fn leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for (i, b) in data.iter().take(8).enumerate() {
        v |= ((b & 0x7F) as u64) << (7 * i);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

/// Split an AV1 low-overhead bitstream (Section 5) into OBUs.
pub fn obus(data: &[u8]) -> Vec<Obu<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let h = data[i];
        let kind = (h >> 3) & 0x0F;
        let ext = h & 0x04 != 0;
        let has_size = h & 0x02 != 0;
        let hlen = if ext { 2 } else { 1 };
        if i + hlen > data.len() {
            break;
        }
        let (size, slen) = if has_size {
            match leb128(&data[i + hlen..]) {
                Some((s, n)) => (s as usize, n),
                None => break,
            }
        } else {
            (data.len() - i - hlen, 0)
        };
        let p0 = i + hlen + slen;
        let p1 = (p0 + size).min(data.len());
        out.push(Obu { kind, whole: &data[i..p1], header: &data[i..i + hlen], payload: &data[p0..p1] });
        i = p1;
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
    fn splits_obus() {
        // temporal delimiter (size 0), sequence header (size 2), frame without size field
        let d = [0x12, 0x00, 0x0A, 0x02, 0xAA, 0xBB, 0x30, 0x01, 0x02];
        let o = obus(&d);
        assert_eq!(o.iter().map(|o| o.kind).collect::<Vec<_>>(), vec![2, 1, 6]);
        assert_eq!(o[1].payload, &[0xAA, 0xBB]);
        assert_eq!(o[2].payload, &[0x01, 0x02]);
    }

    #[test]
    fn trims_to_window_on_keyframes() {
        let b = ClipBuffer::default();
        b.set_video_format(64, 64, 10, VideoCodec::H264);
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
            b.push_video(i * TICKS / 10, d, false);
        }
        let s = b.snapshot(3).unwrap();
        assert!(s.video[0].key);
        assert_eq!(s.params.len(), 2);
        let span = s.video.last().unwrap().pts - s.video[0].pts;
        assert!(span >= 3 * TICKS && span < 4 * TICKS + TICKS, "span {span}");
        // Extending from an earlier start returns the longer range.
        let e = b.snapshot_from(s.video[0].pts - 1).unwrap();
        assert!(e.video[0].pts < s.video[0].pts && e.video.len() > s.video.len());
    }

    #[test]
    fn av1_uses_encoder_keyframes() {
        let b = ClipBuffer::default();
        b.set_video_format(64, 64, 10, VideoCodec::Av1);
        b.set_keep_seconds(3);
        for i in 0..50i64 {
            let key = i % 10 == 0;
            let mut d = vec![0x12, 0x00]; // temporal delimiter
            if key {
                d.extend_from_slice(&[0x0A, 0x01, 0x00]); // sequence header
            }
            d.extend_from_slice(&[0x32, 0x01, 0x00]); // frame
            b.push_video(i * TICKS / 10, d, key);
        }
        let s = b.snapshot(2).unwrap();
        assert!(s.video[0].key && s.params.len() == 1);
    }
}
