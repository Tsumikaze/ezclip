//! Reading and losslessly trimming MP4 clips: any codec (H.264, HEVC, AV1,
//! ...), any recorder. Pure Rust, no Windows calls, so it is testable anywhere.
//!
//! Trimming copies the samples it keeps and the codec description (`stsd`,
//! e.g. avcC/hvcC/av1C) unchanged, and rebuilds only the timing tables, so
//! it never needs to understand the codec itself.
use crate::mp4w::{bx, cat, ftyp, mdat_header, mvhd, Track, TrackKind};
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

pub type Result<T> = std::result::Result<T, String>;

fn io<T>(what: &'static str, r: std::io::Result<T>) -> Result<T> {
    r.map_err(|e| format!("{what}: {e}"))
}

/// What the trim editor needs to know about a clip.
pub struct ClipInfo {
    pub duration: f64,
    /// Times (seconds) where a trim may start without re-encoding.
    pub keyframes: Vec<f64>,
}

/// Duration and video codec straight from the MP4 boxes. Cheap (reads only
/// `moov`), works for every codec, used for the clips list.
pub struct QuickInfo {
    pub duration: f64,
    /// Sample entry of the video track: `avc1`, `hvc1`, `av01`, ...
    pub video_fourcc: Option<[u8; 4]>,
}

impl QuickInfo {
    pub fn codec_name(&self) -> &'static str {
        match self.video_fourcc.as_ref().map(|f| &f[..]) {
            Some(b"avc1") | Some(b"avc3") => "H.264",
            Some(b"hvc1") | Some(b"hev1") => "HEVC",
            Some(b"av01") => "AV1",
            _ => "unknown",
        }
    }
}

fn children(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= b.len() {
        let mut size = u32::from_be_bytes(b[i..i + 4].try_into().unwrap()) as usize;
        let kind: [u8; 4] = b[i + 4..i + 8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 && i + 16 <= b.len() {
            size = u64::from_be_bytes(b[i + 8..i + 16].try_into().unwrap()) as usize;
            hdr = 16;
        } else if size == 0 {
            size = b.len() - i;
        }
        if size < hdr || i + size > b.len() {
            break;
        }
        out.push((kind, &b[i + hdr..i + size]));
        i += size;
    }
    out
}

fn child<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(b).into_iter().find(|(k, _)| k == kind).map(|(_, v)| v)
}

pub fn quick_info(path: &Path) -> Option<QuickInfo> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut pos = 0u64;
    // Walk top-level boxes, skipping mdat, until moov.
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        let mut hdr = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr {
            return None;
        }
        if &h[4..8] == b"moov" {
            if size > 256 << 20 {
                return None;
            }
            let mut moov = vec![0u8; (size - hdr) as usize];
            f.read_exact(&mut moov).ok()?;
            return parse_moov(&moov);
        }
        pos += size;
    }
    None
}

fn parse_moov(moov: &[u8]) -> Option<QuickInfo> {
    let mvhd = child(moov, b"mvhd")?;
    let (ts, dur) = if mvhd.first() == Some(&1) {
        (u32::from_be_bytes(mvhd.get(20..24)?.try_into().ok()?) as f64, u64::from_be_bytes(mvhd.get(24..32)?.try_into().ok()?) as f64)
    } else {
        (u32::from_be_bytes(mvhd.get(12..16)?.try_into().ok()?) as f64, u32::from_be_bytes(mvhd.get(16..20)?.try_into().ok()?) as f64)
    };
    let mut video_fourcc = None;
    for (k, trak) in children(moov) {
        if &k != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else { continue };
        let is_video = child(mdia, b"hdlr").and_then(|h| h.get(8..12)) == Some(&b"vide"[..]);
        if !is_video {
            continue;
        }
        let stsd = child(mdia, b"minf").and_then(|m| child(m, b"stbl")).and_then(|s| child(s, b"stsd"));
        if let Some(entry) = stsd.and_then(|s| s.get(12..16)) {
            video_fourcc = entry.try_into().ok();
        }
        break;
    }
    Some(QuickInfo { duration: if ts > 0.0 { dur / ts } else { 0.0 }, video_fourcc })
}


// ------------------------------------------------------- sample tables ---

#[derive(PartialEq)]
enum Kind {
    Video { width: u32, height: u32 },
    Audio,
    Other,
}

/// One track of a source file, with every sample located.
struct SrcTrack {
    kind: Kind,
    timescale: u32,
    language: u16,
    /// The whole `stsd` box, copied verbatim into the trimmed file.
    stsd: Vec<u8>,
    dts: Vec<u64>,
    durs: Vec<u32>,
    ctts: Option<Vec<i32>>,
    /// `None` = every sample is a keyframe.
    sync: Option<Vec<bool>>,
    sizes: Vec<u32>,
    offsets: Vec<u64>,
}

impl SrcTrack {
    fn secs(&self, t: u64) -> f64 {
        t as f64 / self.timescale.max(1) as f64
    }
    fn is_sync(&self, i: usize) -> bool {
        self.sync.as_ref().map(|s| s[i]).unwrap_or(true)
    }
    fn end_secs(&self) -> f64 {
        match (self.dts.last(), self.durs.last()) {
            (Some(&d), Some(&l)) => self.secs(d + l as u64),
            _ => 0.0,
        }
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Read the `moov` box of an MP4 (skipping `mdat`).
fn read_moov(f: &mut File) -> Result<Vec<u8>> {
    let len = io("read clip", f.metadata())?.len();
    let mut pos = 0u64;
    while pos + 8 <= len {
        io("read clip", f.seek(SeekFrom::Start(pos)))?;
        let mut h = [0u8; 16];
        io("read clip", f.read_exact(&mut h[..8]))?;
        let mut size = u32::from_be_bytes(h[..4].try_into().unwrap()) as u64;
        let mut hdr = 8;
        if size == 1 {
            io("read clip", f.read_exact(&mut h[8..16]))?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            hdr = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < hdr || pos + size > len {
            break;
        }
        if &h[4..8] == b"moov" {
            if size > 512 << 20 {
                return Err("clip index is too large".into());
            }
            let mut moov = vec![0u8; (size - hdr) as usize];
            io("read clip", f.read_exact(&mut moov))?;
            return Ok(moov);
        }
        pos += size;
    }
    Err("not a complete MP4 file (no index found; was it still being recorded?)".into())
}

fn parse_track(trak: &[u8]) -> Option<SrcTrack> {
    let mdia = child(trak, b"mdia")?;
    let mdhd = child(mdia, b"mdhd")?;
    let (timescale, language) = if mdhd.first() == Some(&1) {
        (be32(mdhd, 20)?, u16::from_be_bytes(mdhd.get(32..34)?.try_into().ok()?))
    } else {
        (be32(mdhd, 12)?, u16::from_be_bytes(mdhd.get(20..22)?.try_into().ok()?))
    };
    let handler = child(mdia, b"hdlr").and_then(|h| h.get(8..12))?;
    let kind = match handler {
        b"vide" => {
            let tkhd = child(trak, b"tkhd")?;
            let n = tkhd.len();
            Kind::Video { width: be32(tkhd, n - 8)? >> 16, height: be32(tkhd, n - 4)? >> 16 }
        }
        b"soun" => Kind::Audio,
        _ => Kind::Other,
    };
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    let stsd = bx(b"stsd", child(stbl, b"stsd")?);

    // stts: decode times and durations
    let stts = child(stbl, b"stts")?;
    let (mut dts, mut durs) = (Vec::new(), Vec::new());
    let mut t = 0u64;
    for e in 0..be32(stts, 4)? as usize {
        let (n, d) = (be32(stts, 8 + e * 8)?, be32(stts, 12 + e * 8)?);
        for _ in 0..n {
            dts.push(t);
            durs.push(d);
            t += d as u64;
        }
    }
    let count = dts.len();

    // stsz: sizes
    let stsz = child(stbl, b"stsz")?;
    let uniform = be32(stsz, 4)?;
    let sizes: Vec<u32> = if uniform != 0 {
        vec![uniform; count]
    } else {
        (0..count).map(|i| be32(stsz, 12 + i * 4)).collect::<Option<_>>()?
    };

    // stco / co64 + stsc: where each sample lives
    let chunks: Vec<u64> = if let Some(co) = child(stbl, b"stco") {
        (0..be32(co, 4)? as usize).map(|i| be32(co, 8 + i * 4).map(|v| v as u64)).collect::<Option<_>>()?
    } else {
        let co = child(stbl, b"co64")?;
        (0..be32(co, 4)? as usize).map(|i| be64(co, 8 + i * 8)).collect::<Option<_>>()?
    };
    let stsc = child(stbl, b"stsc")?;
    let runs: Vec<(u32, u32)> = (0..be32(stsc, 4)? as usize)
        .map(|e| Some((be32(stsc, 8 + e * 12)?, be32(stsc, 12 + e * 12)?)))
        .collect::<Option<_>>()?;
    let mut offsets = Vec::with_capacity(count);
    let mut run = 0;
    for (c, &chunk_off) in chunks.iter().enumerate() {
        while run + 1 < runs.len() && runs[run + 1].0 as usize <= c + 1 {
            run += 1;
        }
        let per_chunk = runs.get(run).map(|r| r.1).unwrap_or(1);
        let mut off = chunk_off;
        for _ in 0..per_chunk {
            if offsets.len() >= count {
                break;
            }
            offsets.push(off);
            off += sizes[offsets.len() - 1] as u64;
        }
    }
    if offsets.len() != count || sizes.len() != count {
        return None;
    }

    // ctts (B-frames) and stss (keyframes) are optional
    let ctts = child(stbl, b"ctts").and_then(|b| {
        let mut v = Vec::with_capacity(count);
        for e in 0..be32(b, 4)? as usize {
            let (n, o) = (be32(b, 8 + e * 8)?, be32(b, 12 + e * 8)? as i32);
            v.extend(std::iter::repeat(o).take(n as usize));
        }
        v.resize(count, 0);
        Some(v)
    });
    let sync = child(stbl, b"stss").and_then(|b| {
        let mut v = vec![false; count];
        for e in 0..be32(b, 4)? as usize {
            if let Some(slot) = v.get_mut(be32(b, 8 + e * 4)?.saturating_sub(1) as usize) {
                *slot = true;
            }
        }
        Some(v)
    });
    Some(SrcTrack { kind, timescale, language, stsd, dts, durs, ctts, sync, sizes, offsets })
}

fn read_tracks(f: &mut File) -> Result<Vec<SrcTrack>> {
    let moov = read_moov(f)?;
    if child(&moov, b"mvex").is_some() {
        return Err("fragmented MP4s can't be trimmed".into());
    }
    Ok(children(&moov).into_iter().filter(|(k, _)| k == b"trak").filter_map(|(_, t)| parse_track(t)).collect())
}

fn video_track(tracks: &[SrcTrack]) -> Result<&SrcTrack> {
    tracks
        .iter()
        .find(|t| matches!(t.kind, Kind::Video { .. }) && !t.dts.is_empty())
        .ok_or_else(|| "clip has no video".to_string())
}

pub fn probe(path: &Path) -> Result<ClipInfo> {
    let mut f = io("open clip", File::open(path))?;
    let tracks = read_tracks(&mut f)?;
    let v = video_track(&tracks)?;
    let keyframes = (0..v.dts.len()).filter(|&i| v.is_sync(i)).map(|i| v.secs(v.dts[i])).collect();
    Ok(ClipInfo { duration: v.end_secs(), keyframes })
}

/// Keyframe-accurate lossless cut: the result starts at the last keyframe at
/// or before `start` and ends just before `end` (seconds). `dst` may equal
/// `src` (trim in place). Returns the new duration in seconds.
pub fn trim(src: &Path, dst: &Path, start: f64, end: f64) -> Result<f64> {
    let mut f = io("open clip", File::open(src))?;
    let tracks = read_tracks(&mut f)?;
    let v = video_track(&tracks)?;

    // Video range: from the keyframe at/before `start` up to `end`.
    let first = (0..v.dts.len()).filter(|&i| v.is_sync(i) && v.secs(v.dts[i]) <= start + 1e-6).last().unwrap_or(0);
    let last_excl = (0..v.dts.len()).find(|&i| v.secs(v.dts[i]) >= end - 1e-6).unwrap_or(v.dts.len()).max(first + 1);
    let base_sec = v.secs(v.dts[first]);
    let end_sec = v.dts.get(last_excl).map(|&t| v.secs(t)).unwrap_or(f64::MAX);

    // Keep the video track and every audio track, each cut to the same window.
    let kept: Vec<(&SrcTrack, std::ops::Range<usize>)> = std::iter::once((v, first..last_excl))
        .chain(tracks.iter().filter(|t| t.kind == Kind::Audio).map(|t| {
            let a0 = (0..t.dts.len()).find(|&i| t.secs(t.dts[i]) >= base_sec - 1e-6).unwrap_or(t.dts.len());
            let a1 = (0..t.dts.len()).find(|&i| t.secs(t.dts[i]) >= end_sec - 1e-6).unwrap_or(t.dts.len());
            (t, a0..a1.max(a0))
        }))
        .collect();

    // Write order: interleaved by time (video first on ties).
    let mut order: Vec<(f64, usize, usize)> =
        kept.iter().enumerate().flat_map(|(ti, (t, r))| r.clone().map(move |i| (t.secs(t.dts[i]), ti, i))).collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let tmp = dst.with_extension("mp4.part");
    let result = (|| -> Result<f64> {
        let mut w = BufWriter::with_capacity(1 << 20, io("create file", File::create(&tmp))?);
        let ftyp = ftyp(&[b"isom", b"iso2", b"mp41"]);
        io("write", w.write_all(&ftyp))?;
        let mdat_start = ftyp.len() as u64;
        io("write", w.write_all(&mdat_header()))?;
        let mut pos = mdat_start + 16;

        let mut out: Vec<Track> = kept
            .iter()
            .map(|(t, _)| Track {
                sync: t.sync.as_ref().map(|_| Vec::new()),
                ctts: t.ctts.as_ref().map(|_| Vec::new()),
                ..Default::default()
            })
            .collect();
        let mut buf = Vec::new();
        for &(_, ti, i) in &order {
            let t = kept[ti].0;
            buf.resize(t.sizes[i] as usize, 0);
            io("read clip", f.seek(SeekFrom::Start(t.offsets[i])))?;
            io("read clip", f.read_exact(&mut buf))?;
            io("write", w.write_all(&buf))?;
            let o = &mut out[ti];
            o.push(pos, t.sizes[i], t.durs[i]);
            if let (Some(s), true) = (o.sync.as_mut(), t.is_sync(i)) {
                s.push(o.sizes.len() as u32);
            }
            if let (Some(c), Some(src)) = (o.ctts.as_mut(), t.ctts.as_ref()) {
                c.push(src[i]);
            }
            pos += t.sizes[i] as u64;
        }

        let ms = |t: &SrcTrack, o: &Track| (o.duration() * 1000 / t.timescale.max(1) as u64) as u32;
        let movie_ms = kept.iter().zip(&out).map(|((t, _), o)| ms(t, o)).max().unwrap_or(0);
        let mut moov = vec![mvhd(movie_ms, kept.len() as u32 + 1)];
        for (n, ((t, _), o)) in kept.iter().zip(&out).enumerate() {
            let kind = match t.kind {
                Kind::Video { width, height } => TrackKind::Video { width, height },
                _ => TrackKind::Audio,
            };
            moov.push(o.trak(n as u32 + 1, t.timescale, ms(t, o), kind, t.stsd.clone(), t.language));
        }
        io("write", w.write_all(&bx(b"moov", &cat(&moov))))?;
        io("write", w.seek(SeekFrom::Start(mdat_start + 8)))?;
        io("write", w.write_all(&(pos - mdat_start).to_be_bytes()))?;
        io("write", w.flush())?;
        Ok(out[0].duration() as f64 / v.timescale.max(1) as f64)
    })();
    // Close the source before replacing it (trimming in place).
    drop(f);
    match result {
        Ok(d) => {
            if dst.exists() {
                io("replace clip", std::fs::remove_file(dst))?;
            }
            io("save clip", std::fs::rename(&tmp, dst))?;
            Ok(d)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
