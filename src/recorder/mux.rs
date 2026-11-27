//! Writes a buffer snapshot to an .mp4 file. No re-encoding: the encoded video
//! (H.264, HEVC or AV1) and AAC packets are copied straight into the
//! container, so saving is instant.
//!
//! This is a small, purpose-built MP4 writer: one video track, one optional
//! AAC track, samples interleaved by time, `moov` at the end.
use super::buffer::{nal_units, obus, Snapshot, VideoCodec, TICKS};
use crate::mp4w::{bx, cat, ftyp, full, mdat_header, mvhd, stsd, Track, TrackKind, W};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

pub type Result<T> = std::result::Result<T, String>;

const VIDEO_TS: u32 = 90_000;
const AUDIO_RATE: u32 = 48_000;
pub const AAC_FRAME: i64 = 1024;

fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| e.to_string())
}

// ----------------------------------------------------- codec config boxes ---

/// Remove H.264/HEVC emulation-prevention bytes (00 00 03 -> 00 00).
fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

fn avcc(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let profile = sps.get(1).copied().unwrap_or(100);
    let mut w = W::new()
        .u8(1)
        .u8(profile)
        .u8(sps.get(2).copied().unwrap_or(0))
        .u8(sps.get(3).copied().unwrap_or(41))
        .u8(0xFF) // 4-byte lengths
        .u8(0xE1) // 1 SPS
        .u16(sps.len() as u16)
        .bytes(sps)
        .u8(1) // 1 PPS
        .u16(pps.len() as u16)
        .bytes(pps);
    if matches!(profile, 100 | 110 | 122 | 144) {
        // 4:2:0, 8-bit (what the recorder produces), no SPS extensions.
        w = w.u8(0xFC | 1).u8(0xF8).u8(0xF8).u8(0);
    }
    bx(b"avcC", &w.0)
}

fn hvcc(vps: &[u8], sps: &[u8], pps: &[u8]) -> Vec<u8> {
    // profile_tier_level follows the 2-byte NAL header and 1 byte of SPS fields.
    let rbsp = unescape(sps.get(2..).unwrap_or(&[]));
    let mut ptl = [0u8; 12];
    if rbsp.len() >= 13 {
        ptl.copy_from_slice(&rbsp[1..13]);
    } else {
        ptl[0] = 1; // Main profile
        ptl[11] = 120; // level 4
    }
    let max_sub_layers = rbsp.first().map(|b| ((b >> 1) & 0x07) + 1).unwrap_or(1);
    let nested = rbsp.first().map(|b| b & 1).unwrap_or(1);
    let mut w = W::new()
        .u8(1)
        .bytes(&ptl) // profile space/tier/idc, compat flags, constraint flags, level
        .u16(0xF000) // min_spatial_segmentation_idc
        .u8(0xFC) // parallelismType
        .u8(0xFC | 1) // chroma 4:2:0
        .u8(0xF8) // luma 8-bit
        .u8(0xF8) // chroma 8-bit
        .u16(0) // avgFrameRate
        .u8((max_sub_layers << 3) | (nested << 2) | 3) // 4-byte lengths
        .u8(3);
    for (kind, nal) in [(32u8, vps), (33, sps), (34, pps)] {
        w = w.u8(0x80 | kind).u16(1).u16(nal.len() as u16).bytes(nal);
    }
    bx(b"hvcC", &w.0)
}

/// Bit reader for the AV1 sequence header.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}
impl Bits<'_> {
    fn f(&mut self, n: usize) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            let byte = self.d.get(self.pos / 8).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        v
    }
    fn uvlc(&mut self) -> u32 {
        let mut zeros = 0;
        while self.f(1) == 0 && zeros < 32 {
            zeros += 1;
        }
        if zeros >= 32 {
            return u32::MAX;
        }
        self.f(zeros)
    }
}

/// av1C needs the profile, level and tier from the sequence header.
fn av1c(seq_obu: &[u8]) -> Vec<u8> {
    let payload = obus(seq_obu).first().map(|o| o.payload).unwrap_or(&[]);
    let mut b = Bits { d: payload, pos: 0 };
    let profile = b.f(3);
    let _still = b.f(1);
    let reduced = b.f(1);
    let (mut level, mut tier) = (0, 0);
    if reduced == 1 {
        level = b.f(5);
    } else {
        let timing = b.f(1);
        let mut decoder_model = 0;
        let mut delay_len = 0;
        if timing == 1 {
            b.f(32);
            b.f(32);
            if b.f(1) == 1 {
                b.uvlc();
            }
            decoder_model = b.f(1);
            if decoder_model == 1 {
                delay_len = b.f(5) + 1;
                b.f(32);
                b.f(5);
                b.f(5);
            }
        }
        let initial_delay = b.f(1);
        let ops = b.f(5) + 1;
        for i in 0..ops {
            b.f(12);
            let l = b.f(5);
            let t = if l > 7 { b.f(1) } else { 0 };
            if i == 0 {
                level = l;
                tier = t;
            }
            if decoder_model == 1 && b.f(1) == 1 {
                b.f(delay_len as usize);
                b.f(delay_len as usize);
                b.f(1);
            }
            if initial_delay == 1 && b.f(1) == 1 {
                b.f(4);
            }
        }
    }
    let body = W::new()
        .u8(0x81) // marker + version 1
        .u8(((profile as u8) << 5) | level as u8)
        // tier, 8-bit, not mono, 4:2:0 (subsampling x/y = 1), chroma position 0
        .u8(((tier as u8) << 7) | 0b0000_1100)
        .u8(0)
        .bytes(seq_obu);
    bx(b"av1C", &body.0)
}

/// BT.709 limited range, matching the recorder's colour conversion.
fn colr() -> Vec<u8> {
    bx(b"colr", &W::new().bytes(b"nclx").u16(1).u16(1).u16(1).u8(0).0)
}

fn visual_entry(fourcc: &[u8; 4], w: u32, h: u32, config: Vec<u8>) -> Vec<u8> {
    let body = W::new()
        .zeros(6)
        .u16(1) // data_reference_index
        .zeros(16)
        .u16(w as u16)
        .u16(h as u16)
        .u32(0x0048_0000)
        .u32(0x0048_0000)
        .u32(0)
        .u16(1) // frame_count
        .zeros(32) // compressorname
        .u16(0x0018)
        .u16(0xFFFF)
        .bytes(&config)
        .bytes(&colr());
    bx(fourcc, &body.0)
}

fn mp4a_entry() -> Vec<u8> {
    // AudioSpecificConfig: AAC-LC (2), 48 kHz (index 3), stereo (2).
    let asc = [0x11u8, 0x90];
    let dsi = cat(&[vec![0x05, asc.len() as u8], asc.to_vec()]);
    let dcd_body = W::new().u8(0x40).u8(0x15).bytes(&[0, 0, 0]).u32(192_000).u32(192_000).bytes(&dsi).0;
    let dcd = cat(&[vec![0x04, dcd_body.len() as u8], dcd_body]);
    let sl = vec![0x06, 0x01, 0x02];
    let es_body = cat(&[W::new().u16(1).u8(0).0, dcd, sl]);
    let es = cat(&[vec![0x03, es_body.len() as u8], es_body]);
    let esds = full(b"esds", 0, 0, &es);
    let body = W::new()
        .zeros(6)
        .u16(1)
        .zeros(8)
        .u16(2) // channels
        .u16(16) // sample size
        .u32(0)
        .u32(AUDIO_RATE << 16)
        .bytes(&esds);
    bx(b"mp4a", &body.0)
}

// ------------------------------------------------------------ sample data ---

/// Encoder output -> MP4 sample format for the codec.
fn sample_bytes(codec: VideoCodec, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 16);
    match codec {
        VideoCodec::H264 | VideoCodec::Hevc => {
            for nal in nal_units(data) {
                let drop = match codec {
                    // Parameter sets live in avcC/hvcC; access unit delimiters are noise.
                    VideoCodec::H264 => matches!(nal[0] & 0x1F, 7 | 8 | 9),
                    _ => matches!((nal[0] >> 1) & 0x3F, 32..=35),
                };
                if !drop {
                    out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                    out.extend_from_slice(nal);
                }
            }
        }
        VideoCodec::Av1 => {
            // MP4 samples carry OBUs with size fields and no temporal delimiters.
            for o in obus(data) {
                if o.kind == 2 || o.kind == 15 {
                    continue;
                }
                let mut h = o.header.to_vec();
                h[0] |= 0x02; // has_size_field
                out.extend_from_slice(&h);
                let mut n = o.payload.len();
                loop {
                    let byte = (n & 0x7F) as u8;
                    n >>= 7;
                    out.push(if n > 0 { byte | 0x80 } else { byte });
                    if n == 0 {
                        break;
                    }
                }
                out.extend_from_slice(o.payload);
            }
        }
    }
    out
}

// ------------------------------------------------------------------ write ---

/// Returns the clip duration in seconds.
pub fn write_mp4(path: &Path, snap: &Snapshot) -> Result<f64> {
    if snap.video.is_empty() {
        return Err("nothing to save".into());
    }
    let (fourcc, config): (&[u8; 4], Vec<u8>) = match snap.codec {
        VideoCodec::H264 => (b"avc1", avcc(&snap.params[0], &snap.params[1])),
        VideoCodec::Hevc => (b"hvc1", hvcc(&snap.params[0], &snap.params[1], &snap.params[2])),
        VideoCodec::Av1 => (b"av01", av1c(&snap.params[0])),
    };

    let mut f = BufWriter::with_capacity(1 << 20, io(File::create(path))?);
    let ftyp = ftyp(&[b"isom", b"iso2", fourcc, b"mp41"]);
    io(f.write_all(&ftyp))?;
    // 64-bit mdat header; the size is filled in at the end.
    let mdat_start = ftyp.len() as u64;
    io(f.write_all(&mdat_header()))?;
    let mut pos = mdat_start + 16;

    let t0 = snap.video[0].pts;
    let vt = |pts: i64| ((pts - t0).max(0) * VIDEO_TS as i64 + TICKS / 2) / TICKS;
    let frame = (VIDEO_TS as i64 / snap.fps.max(1) as i64).max(1);

    let mut video = Track { sync: Some(Vec::new()), ..Default::default() };
    let mut audio = Track::default();
    let (mut vi, mut ai) = (0usize, 0usize);
    let (n_v, n_a) = (snap.video.len(), snap.audio.len());
    // Interleave the two tracks by time so players can stream the file.
    while vi < n_v || ai < n_a {
        let v_time = snap.video.get(vi).map(|p| p.pts);
        let a_time = snap.audio.get(ai).map(|_| t0 + (ai as i64 * AAC_FRAME * TICKS) / AUDIO_RATE as i64);
        let take_video = match (v_time, a_time) {
            (Some(v), Some(a)) => v <= a,
            (Some(_), None) => true,
            _ => false,
        };
        if take_video {
            let p = &snap.video[vi];
            let data = sample_bytes(snap.codec, &p.data);
            let start = vt(p.pts);
            let next = snap.video.get(vi + 1).map(|n| vt(n.pts)).unwrap_or(start + frame);
            io(f.write_all(&data))?;
            video.push(pos, data.len() as u32, (next - start).max(1) as u32);
            if p.key {
                video.sync.as_mut().unwrap().push(vi as u32 + 1);
            }
            pos += data.len() as u64;
            vi += 1;
        } else {
            let p = &snap.audio[ai];
            io(f.write_all(&p.data))?;
            audio.push(pos, p.data.len() as u32, AAC_FRAME as u32);
            pos += p.data.len() as u64;
            ai += 1;
        }
    }

    let v_ms = (video.duration() * 1000 / VIDEO_TS as u64) as u32;
    let a_ms = (audio.duration() * 1000 / AUDIO_RATE as u64) as u32;
    let has_audio = !audio.sizes.is_empty();
    let video_kind = TrackKind::Video { width: snap.width, height: snap.height };
    let video_stsd = stsd(visual_entry(fourcc, snap.width, snap.height, config));
    let mut moov = vec![mvhd(v_ms.max(a_ms), if has_audio { 3 } else { 2 }), video.trak(1, VIDEO_TS, v_ms, video_kind, video_stsd, 0x55C4)];
    if has_audio {
        moov.push(audio.trak(2, AUDIO_RATE, a_ms, TrackKind::Audio, stsd(mp4a_entry()), 0x55C4));
    }
    io(f.write_all(&bx(b"moov", &cat(&moov))))?;

    // Patch the mdat size.
    let mdat_size = pos - mdat_start;
    io(f.seek(SeekFrom::Start(mdat_start + 8)))?;
    io(f.write_all(&mdat_size.to_be_bytes()))?;
    io(f.flush())?;
    Ok(video.duration() as f64 / VIDEO_TS as f64)
}
