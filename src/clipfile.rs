//! Reading and losslessly trimming MP4 clips (pure Rust, no Windows calls, so
//! it is testable anywhere). Only H.264 video + AAC audio are supported, which
//! covers ezclip's own clips and most other recorders' MP4s.
use bytes::Bytes;
use mp4::{MediaType, Mp4Reader, Mp4Sample, Mp4Track, TrackType};
use std::io::{BufReader, BufWriter};
use std::path::Path;

pub type Result<T> = std::result::Result<T, String>;

fn err<E: std::fmt::Display>(what: &'static str) -> impl Fn(E) -> String {
    move |e| format!("{what}: {e}")
}

/// What the trim editor needs to know about a clip.
pub struct ClipInfo {
    pub duration: f64,
    /// Times (seconds) where a trim may start without re-encoding.
    pub keyframes: Vec<f64>,
}

/// Decode-order timestamps (track timescale) of every sample, from `stts`.
fn sample_times(track: &Mp4Track) -> (Vec<u64>, Vec<u32>) {
    let mut times = Vec::with_capacity(track.sample_count() as usize);
    let mut durs = Vec::with_capacity(track.sample_count() as usize);
    let mut t = 0u64;
    for e in &track.trak.mdia.minf.stbl.stts.entries {
        for _ in 0..e.sample_count {
            times.push(t);
            durs.push(e.sample_delta);
            t += e.sample_delta as u64;
        }
    }
    (times, durs)
}

/// 0-based indices of sync (key) samples.
fn sync_samples(track: &Mp4Track, count: usize) -> Vec<usize> {
    match &track.trak.mdia.minf.stbl.stss {
        Some(stss) => stss.entries.iter().map(|&n| n.saturating_sub(1) as usize).filter(|&i| i < count).collect(),
        None => (0..count).collect(),
    }
}

fn open(path: &Path) -> Result<Mp4Reader<BufReader<std::fs::File>>> {
    let f = std::fs::File::open(path).map_err(err("open clip"))?;
    let size = f.metadata().map_err(err("read clip"))?.len();
    Mp4Reader::read_header(BufReader::new(f), size).map_err(err("not a readable MP4"))
}

fn find_tracks(r: &Mp4Reader<BufReader<std::fs::File>>) -> Result<(u32, Option<u32>)> {
    let mut video = None;
    let mut audio = None;
    let mut ids: Vec<u32> = r.tracks().keys().copied().collect();
    ids.sort();
    for id in ids {
        let t = &r.tracks()[&id];
        match t.track_type() {
            Ok(TrackType::Video) if video.is_none() => {
                if !matches!(t.media_type(), Ok(MediaType::H264)) {
                    return Err("only H.264 clips can be trimmed".into());
                }
                video = Some(id);
            }
            Ok(TrackType::Audio) if audio.is_none() && matches!(t.media_type(), Ok(MediaType::AAC)) => audio = Some(id),
            _ => {}
        }
    }
    Ok((video.ok_or("clip has no video track")?, audio))
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
    pub fn is_h264(&self) -> bool {
        self.codec_name() == "H.264"
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

pub fn probe(path: &Path) -> Result<ClipInfo> {
    if let Some(q) = quick_info(path) {
        if q.video_fourcc.is_some() && !q.is_h264() {
            return Err(format!(
                "Trimming works on H.264 clips only, and this one is {}. Shrink for Discord still works on it.",
                q.codec_name()
            ));
        }
    }
    let r = open(path)?;
    let (vid, _) = find_tracks(&r)?;
    let t = &r.tracks()[&vid];
    let ts = t.timescale().max(1) as f64;
    let (times, durs) = sample_times(t);
    let keyframes = sync_samples(t, times.len()).into_iter().map(|i| times[i] as f64 / ts).collect();
    let duration = times.last().map(|&l| (l + *durs.last().unwrap() as u64) as f64 / ts).unwrap_or(0.0);
    Ok(ClipInfo { duration, keyframes })
}

/// Keyframe-accurate lossless cut: the result starts at the last keyframe at
/// or before `start` and ends just before `end` (seconds). Returns the new
/// duration in seconds.
pub fn trim(src: &Path, dst: &Path, start: f64, end: f64) -> Result<f64> {
    let mut r = open(src)?;
    let (vid, aid) = find_tracks(&r)?;

    let (vconf, vts, vtimes, vdurs, vsync) = {
        let t = &r.tracks()[&vid];
        let (times, durs) = sample_times(t);
        let sync = sync_samples(t, times.len());
        let conf = mp4::AvcConfig {
            width: t.width(),
            height: t.height(),
            seq_param_set: t.sequence_parameter_set().map_err(err("video config"))?.to_vec(),
            pic_param_set: t.picture_parameter_set().map_err(err("video config"))?.to_vec(),
        };
        (conf, t.timescale().max(1), times, durs, sync)
    };
    if vtimes.is_empty() {
        return Err("clip has no video".into());
    }
    let to_ts = |s: f64, ts: u32| (s.max(0.0) * ts as f64).round() as u64;

    let start_t = to_ts(start, vts);
    let end_t = to_ts(end, vts);
    let first = vsync.iter().copied().filter(|&i| vtimes[i] <= start_t).last().unwrap_or(vsync.first().copied().unwrap_or(0));
    let last_excl = vtimes.iter().position(|&t| t >= end_t).unwrap_or(vtimes.len()).max(first + 1);
    let base_v = vtimes[first];
    let base_sec = base_v as f64 / vts as f64;
    let end_sec = vtimes.get(last_excl).map(|&t| t as f64 / vts as f64).unwrap_or(f64::MAX);

    let audio = match aid {
        Some(id) => {
            let t = &r.tracks()[&id];
            let conf = mp4::AacConfig {
                bitrate: t.bitrate(),
                profile: t.audio_profile().map_err(err("audio config"))?,
                freq_index: t.sample_freq_index().map_err(err("audio config"))?,
                chan_conf: t.channel_config().map_err(err("audio config"))?,
            };
            let ts = t.timescale().max(1);
            let (times, durs) = sample_times(t);
            let a0 = times.iter().position(|&x| x as f64 / ts as f64 >= base_sec).unwrap_or(times.len());
            let a1 = times.iter().position(|&x| x as f64 / ts as f64 >= end_sec).unwrap_or(times.len());
            Some((id, conf, ts, times, durs, a0, a1))
        }
        None => None,
    };

    let tmp = dst.with_extension("mp4.part");
    let file = std::fs::File::create(&tmp).map_err(err("create file"))?;
    let result = (|| -> Result<f64> {
        let config = mp4::Mp4Config {
            major_brand: str::parse("isom").unwrap(),
            minor_version: 512,
            compatible_brands: ["isom", "iso2", "avc1", "mp41"].iter().map(|b| str::parse(b).unwrap()).collect(),
            timescale: 1000,
        };
        let mut w = mp4::Mp4Writer::write_start(BufWriter::with_capacity(1 << 20, file), &config).map_err(err("mp4 start"))?;
        w.add_track(&mp4::TrackConfig {
            track_type: TrackType::Video,
            timescale: vts,
            language: "und".into(),
            media_conf: mp4::MediaConfig::AvcConfig(vconf),
        })
        .map_err(err("video track"))?;
        if let Some((_, conf, ts, ..)) = &audio {
            w.add_track(&mp4::TrackConfig {
                track_type: TrackType::Audio,
                timescale: *ts,
                language: "und".into(),
                media_conf: mp4::MediaConfig::AacConfig(conf.clone()),
            })
            .map_err(err("audio track"))?;
        }

        // Interleave by time, like the recorder's muxer.
        let mut vi = first;
        let mut ai = audio.as_ref().map(|a| a.5).unwrap_or(0);
        loop {
            let v_sec = (vi < last_excl).then(|| vtimes[vi] as f64 / vts as f64);
            let a_sec = audio.as_ref().and_then(|a| (ai < a.6).then(|| a.3[ai] as f64 / a.2 as f64));
            let take_video = match (v_sec, a_sec) {
                (Some(v), Some(a)) => v <= a,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            if take_video {
                let s = r.read_sample(vid, vi as u32 + 1).map_err(err("read video"))?.ok_or("missing video sample")?;
                w.write_sample(
                    1,
                    &Mp4Sample {
                        start_time: vtimes[vi] - base_v,
                        duration: vdurs[vi],
                        rendering_offset: s.rendering_offset,
                        is_sync: s.is_sync,
                        bytes: Bytes::from(s.bytes.to_vec()),
                    },
                )
                .map_err(err("write video"))?;
                vi += 1;
            } else {
                let (id, _, ts, times, durs, a0, _) = audio.as_ref().unwrap();
                let base_a = (base_sec * *ts as f64).round() as u64;
                let s = r.read_sample(*id, ai as u32 + 1).map_err(err("read audio"))?.ok_or("missing audio sample")?;
                let _ = a0;
                w.write_sample(
                    2,
                    &Mp4Sample {
                        start_time: times[ai].saturating_sub(base_a),
                        duration: durs[ai],
                        rendering_offset: 0,
                        is_sync: true,
                        bytes: Bytes::from(s.bytes.to_vec()),
                    },
                )
                .map_err(err("write audio"))?;
                ai += 1;
            }
        }
        w.write_end().map_err(err("finish mp4"))?;
        let last = last_excl - 1;
        Ok((vtimes[last] + vdurs[last] as u64 - base_v) as f64 / vts as f64)
    })();
    // Close the source before replacing it (trimming in place).
    drop(r);
    match result {
        Ok(d) => {
            if dst.exists() {
                std::fs::remove_file(dst).map_err(err("replace clip"))?;
            }
            std::fs::rename(&tmp, dst).map_err(err("save clip"))?;
            Ok(d)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}
