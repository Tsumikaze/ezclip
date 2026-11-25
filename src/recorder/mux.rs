//! Writes a buffer snapshot to an .mp4 file. No re-encoding: the H.264 and
//! AAC packets are copied straight into the container, so saving is instant.
use super::buffer::{nal_units, Snapshot, TICKS};
use anyhow_lite::{Context, Result};
use bytes::Bytes;
use mp4::*;
use std::io::BufWriter;
use std::path::Path;

pub mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, String>;
    pub trait Context<T> {
        fn context(self, what: &str) -> Result<T>;
    }
    impl<T, E: std::fmt::Display> Context<T> for std::result::Result<T, E> {
        fn context(self, what: &str) -> Result<T> {
            self.map_err(|e| format!("{what}: {e}"))
        }
    }
}

const VIDEO_TS: i64 = 90_000;
const AUDIO_RATE: i64 = 48_000;
pub const AAC_FRAME: i64 = 1024;

fn to_avcc(annexb: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(annexb.len() + 16);
    for nal in nal_units(annexb) {
        // SPS/PPS live in the avcC box; access unit delimiters are noise.
        if matches!(nal[0] & 0x1F, 7 | 8 | 9) {
            continue;
        }
        out.extend_from_slice(&(nal.len() as u32).to_be_bytes());
        out.extend_from_slice(nal);
    }
    out
}

/// Returns the clip duration in seconds.
pub fn write_mp4(path: &Path, snap: &Snapshot) -> Result<f64> {
    let file = std::fs::File::create(path).context("create file")?;
    let config = Mp4Config {
        major_brand: str::parse("isom").unwrap(),
        minor_version: 512,
        compatible_brands: vec![
            str::parse("isom").unwrap(),
            str::parse("iso2").unwrap(),
            str::parse("avc1").unwrap(),
            str::parse("mp41").unwrap(),
        ],
        timescale: 1000,
    };
    let mut w = Mp4Writer::write_start(BufWriter::with_capacity(1 << 20, file), &config).context("mp4 start")?;

    w.add_track(&TrackConfig {
        track_type: TrackType::Video,
        timescale: VIDEO_TS as u32,
        language: "und".into(),
        media_conf: MediaConfig::AvcConfig(AvcConfig {
            width: snap.width as u16,
            height: snap.height as u16,
            seq_param_set: snap.sps.clone(),
            pic_param_set: snap.pps.clone(),
        }),
    })
    .context("video track")?;

    let has_audio = !snap.audio.is_empty();
    if has_audio {
        w.add_track(&TrackConfig {
            track_type: TrackType::Audio,
            timescale: AUDIO_RATE as u32,
            language: "und".into(),
            media_conf: MediaConfig::AacConfig(AacConfig {
                bitrate: 192_000,
                profile: AudioObjectType::AacLowComplexity,
                freq_index: SampleFreqIndex::Freq48000,
                chan_conf: ChannelConfig::Stereo,
            }),
        })
        .context("audio track")?;
    }

    let t0 = snap.video[0].pts;
    let vt = |pts: i64| ((pts - t0).max(0) * VIDEO_TS + TICKS / 2) / TICKS;
    let frame = (VIDEO_TS / snap.fps.max(1) as i64).max(1);

    // Interleave the two tracks by time so players can stream the file.
    let (mut vi, mut ai) = (0usize, 0usize);
    let n_v = snap.video.len();
    let n_a = snap.audio.len();
    while vi < n_v || ai < n_a {
        let v_time = snap.video.get(vi).map(|p| p.pts);
        let a_time = snap.audio.get(ai).map(|_| t0 + (ai as i64 * AAC_FRAME * TICKS) / AUDIO_RATE);
        let take_video = match (v_time, a_time) {
            (Some(v), Some(a)) => v <= a,
            (Some(_), None) => true,
            _ => false,
        };
        if take_video {
            let p = &snap.video[vi];
            let start = vt(p.pts);
            let next = snap.video.get(vi + 1).map(|n| vt(n.pts)).unwrap_or(start + frame);
            w.write_sample(
                1,
                &Mp4Sample {
                    start_time: start as u64,
                    duration: (next - start).max(1) as u32,
                    rendering_offset: 0,
                    is_sync: p.key,
                    bytes: Bytes::from(to_avcc(&p.data)),
                },
            )
            .context("write video")?;
            vi += 1;
        } else {
            let p = &snap.audio[ai];
            w.write_sample(
                2,
                &Mp4Sample {
                    start_time: (ai as i64 * AAC_FRAME) as u64,
                    duration: AAC_FRAME as u32,
                    rendering_offset: 0,
                    is_sync: true,
                    bytes: Bytes::copy_from_slice(&p.data),
                },
            )
            .context("write audio")?;
            ai += 1;
        }
    }
    w.write_end().context("finish mp4")?;
    let dur = (snap.video.last().unwrap().pts - t0) as f64 / TICKS as f64 + 1.0 / snap.fps.max(1) as f64;
    Ok(dur)
}
