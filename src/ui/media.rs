//! Media Foundation helpers for the window process:
//! * `FrameGrabber`: decodes single frames for the trim editor preview.
//! * `shrink`: re-encodes a clip to fit a size limit ("Shrink for Discord").
use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use windows::core::{Interface, Result, GUID, HSTRING, PROPVARIANT};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

const MF_VERSION: u32 = 0x0002_0070;
const FIRST_VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const FIRST_AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL_STREAMS: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const ANY_STREAM: u32 = MF_SOURCE_READER_ANY_STREAM.0 as u32;
const MEDIASOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

fn mf_thread_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, 0);
    }
}

fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

fn attrs(pairs: &[(GUID, u32)], guids: &[(GUID, GUID)]) -> Result<IMFAttributes> {
    unsafe {
        let mut a = None;
        MFCreateAttributes(&mut a, (pairs.len() + guids.len()) as u32)?;
        let a = a.unwrap();
        for (k, v) in pairs {
            a.SetUINT32(k, *v)?;
        }
        for (k, v) in guids {
            a.SetGUID(k, v)?;
        }
        Ok(a)
    }
}

fn duration_100ns(reader: &IMFSourceReader) -> Option<u64> {
    unsafe {
        let v = reader.GetPresentationAttribute(MEDIASOURCE, &MF_PD_DURATION).ok()?;
        u64::try_from(&v).ok()
    }
}

// ============================================================ frames ===

pub struct FrameRequest {
    pub path: PathBuf,
    pub seconds: f64,
    /// false: nearest keyframe at or before (fast); true: the exact frame.
    pub exact: bool,
    pub id: u64,
}

pub struct Frame {
    pub id: u64,
    pub image: Option<egui::ColorImage>,
}

pub struct FrameGrabber {
    tx: Sender<FrameRequest>,
    rx: Receiver<Frame>,
}

impl FrameGrabber {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, worker_rx) = channel::<FrameRequest>();
        let (worker_tx, rx) = channel::<Frame>();
        std::thread::Builder::new()
            .name("frames".into())
            .spawn(move || {
                mf_thread_init();
                let mut open: Option<(PathBuf, Reader)> = None;
                while let Ok(mut req) = worker_rx.recv() {
                    // Only the newest request matters while the user drags.
                    while let Ok(r) = worker_rx.try_recv() {
                        req = r;
                    }
                    if open.as_ref().map(|(p, _)| p != &req.path).unwrap_or(true) {
                        open = Reader::open(&req.path).ok().map(|r| (req.path.clone(), r));
                    }
                    let image = open.as_mut().and_then(|(_, r)| r.grab(req.seconds, req.exact).ok());
                    if worker_tx.send(Frame { id: req.id, image }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .unwrap();
        Self { tx, rx }
    }

    pub fn request(&self, r: FrameRequest) {
        let _ = self.tx.send(r);
    }

    pub fn poll(&self) -> Option<Frame> {
        let mut last = None;
        while let Ok(f) = self.rx.try_recv() {
            last = Some(f);
        }
        last
    }
}

struct Reader {
    reader: IMFSourceReader,
    width: u32,
    height: u32,
    crop: (u32, u32, u32, u32), // x, y, w, h of the visible picture
    frame_100ns: i64,
}

impl Reader {
    fn open(path: &Path) -> Result<Self> {
        unsafe {
            let a = attrs(&[(MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)], &[])?;
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &a)?;
            reader.SetStreamSelection(ALL_STREAMS, false)?;
            reader.SetStreamSelection(FIRST_VIDEO, true)?;
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            reader.SetCurrentMediaType(FIRST_VIDEO, None, &t)?;
            let cur = reader.GetCurrentMediaType(FIRST_VIDEO)?;
            let (width, height) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE)?);
            let mut crop = (0, 0, width, height);
            // Decoders often pad to 16 lines (1080 -> 1088); the aperture says what's real.
            let mut blob = [0u8; 16];
            if cur.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut blob, None).is_ok() {
                let x = i16::from_le_bytes([blob[2], blob[3]]).max(0) as u32;
                let y = i16::from_le_bytes([blob[6], blob[7]]).max(0) as u32;
                let w = i32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]).max(0) as u32;
                let h = i32::from_le_bytes([blob[12], blob[13], blob[14], blob[15]]).max(0) as u32;
                if w > 0 && h > 0 && x + w <= width && y + h <= height {
                    crop = (x, y, w, h);
                }
            }
            let (num, den) = cur.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).unwrap_or((60, 1));
            let frame_100ns = (10_000_000i64 * den.max(1) as i64 / num.max(1) as i64).max(1);
            Ok(Self { reader, width, height, crop, frame_100ns })
        }
    }

    fn grab(&mut self, seconds: f64, exact: bool) -> Result<egui::ColorImage> {
        unsafe {
            let target = (seconds.max(0.0) * 1e7) as i64;
            self.reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(target))?;
            let mut best: Option<IMFSample> = None;
            for _ in 0..900 {
                let mut flags = 0u32;
                let mut ts = 0i64;
                let mut sample = None;
                self.reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
                if let Some(s) = sample {
                    let done = !exact || ts + self.frame_100ns / 2 >= target;
                    best = Some(s);
                    if done {
                        break;
                    }
                }
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
            }
            let s = best.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            self.to_image(&s)
        }
    }

    unsafe fn to_image(&self, s: &IMFSample) -> Result<egui::ColorImage> {
        let buf = s.ConvertToContiguousBuffer()?;
        let (w, h) = (self.width as usize, self.height as usize);
        let mut scan0 = std::ptr::null_mut();
        let mut pitch = (w * 4) as i32;
        let two_d = buf.cast::<IMF2DBuffer>().ok();
        let mut locked_2d = false;
        if let Some(b) = &two_d {
            if b.Lock2D(&mut scan0, &mut pitch).is_ok() {
                locked_2d = true;
            }
        }
        if !locked_2d {
            let mut len = 0u32;
            buf.Lock(&mut scan0, None, Some(&mut len))?;
            if (len as usize) < w * h * 4 {
                let _ = buf.Unlock();
                return Err(windows::Win32::Foundation::E_FAIL.into());
            }
        }
        let (cx, cy, cw, ch) = (self.crop.0 as usize, self.crop.1 as usize, self.crop.2 as usize, self.crop.3 as usize);
        // Box-downscale to at most 960 px wide.
        let scale = (cw as f64 / 960.0).ceil().max(1.0) as usize;
        let (ow, oh) = (cw / scale, ch / scale);
        let mut px = vec![0u8; ow * oh * 4];
        for oy in 0..oh {
            for ox in 0..ow {
                let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
                for dy in 0..scale {
                    let row = scan0.offset((cy + oy * scale + dy) as isize * pitch as isize);
                    for dx in 0..scale {
                        let p = row.add((cx + ox * scale + dx) * 4);
                        b += *p as u32;
                        g += *p.add(1) as u32;
                        r += *p.add(2) as u32;
                    }
                }
                let n = (scale * scale) as u32;
                let i = (oy * ow + ox) * 4;
                px[i] = (r / n) as u8;
                px[i + 1] = (g / n) as u8;
                px[i + 2] = (b / n) as u8;
                px[i + 3] = 255;
            }
        }
        if locked_2d {
            let _ = two_d.unwrap().Unlock2D();
        } else {
            let _ = buf.Unlock();
        }
        Ok(egui::ColorImage::from_rgba_unmultiplied([ow, oh], &px))
    }
}

// ============================================================ shrink ===

pub enum ShrinkUpdate {
    Progress(PathBuf, f32),
    /// Ok(path of the small file; may be the original if it already fits).
    Done(PathBuf, std::result::Result<PathBuf, String>),
}

/// Re-encode `src` to fit in `limit_mb` on a background thread.
pub fn shrink(src: PathBuf, limit_mb: u32, tx: Sender<ShrinkUpdate>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("shrink".into())
        .spawn(move || {
            mf_thread_init();
            let progress = |f: f32| {
                let _ = tx.send(ShrinkUpdate::Progress(src.clone(), f));
                ctx.request_repaint();
            };
            let r = shrink_blocking(&src, limit_mb as u64 * 1_000_000, &progress);
            if let Err(e) = &r {
                crate::log::write(&format!("shrink {}: {e}", src.display()));
            }
            let _ = tx.send(ShrinkUpdate::Done(src.clone(), r));
            ctx.request_repaint();
        })
        .unwrap();
}

struct Plan {
    width: u32,
    height: u32,
    fps: u32,
    /// Keep every n-th source frame.
    step: u32,
    video_bps: u32,
    audio_bytes_per_sec: u32,
}

fn shrink_blocking(src: &Path, limit: u64, progress: &dyn Fn(f32)) -> std::result::Result<PathBuf, String> {
    let size = std::fs::metadata(src).map_err(|e| e.to_string())?.len();
    if size <= limit {
        return Ok(src.to_path_buf()); // already fits
    }
    let (src_w, src_h, src_fps, dur) = unsafe { probe(src) }.map_err(|e| format!("can't read clip: {e}"))?;
    let secs = (dur as f64 / 1e7).max(0.5);
    let audio_bytes_per_sec = if limit <= 10_000_000 { 12_000 } else { 16_000 }; // 96 / 128 kbps
    // 8 % headroom for the container and encoder overshoot.
    let mut video_bps = (limit as f64 * 8.0 * 0.92 / secs) - audio_bytes_per_sec as f64 * 8.0;
    if video_bps < 150_000.0 {
        return Err(format!("{:.0} s is too long for {} MB — trim it first", secs, limit / 1_000_000));
    }

    let stem = src.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let dst = src.with_file_name(format!("{stem} (discord).mp4"));
    let tmp = dst.with_extension("mp4.part");

    for _attempt in 0..3 {
        let p = plan(src_w, src_h, src_fps, video_bps as u32, audio_bytes_per_sec);
        unsafe { encode(src, &tmp, &p, dur, progress) }.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("encoding failed: {e}")
        })?;
        let got = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(u64::MAX);
        if got <= limit {
            if dst.exists() {
                let _ = std::fs::remove_file(&dst);
            }
            std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
            crate::clipmeta::mark_ezclip(&dst);
            return Ok(dst);
        }
        // Encoder overshot: aim lower in proportion and try again.
        video_bps *= limit as f64 / got as f64 * 0.92;
    }
    let _ = std::fs::remove_file(&tmp);
    Err("couldn't get the clip small enough — try trimming it".into())
}

fn plan(w: u32, h: u32, fps: u32, video_bps: u32, audio_bytes_per_sec: u32) -> Plan {
    let max_h = match video_bps {
        b if b >= 9_000_000 => 1440,
        b if b >= 5_000_000 => 1080,
        b if b >= 2_500_000 => 720,
        b if b >= 1_200_000 => 540,
        _ => 360,
    };
    let out_h = h.min(max_h).max(2) & !1;
    let out_w = ((w as u64 * out_h as u64 / h.max(1) as u64) as u32).max(2) & !1;
    let cap = if video_bps < 3_000_000 { 30 } else { 60 };
    let step = fps.div_ceil(cap).max(1);
    Plan { width: out_w, height: out_h, fps: (fps / step).max(1), step, video_bps, audio_bytes_per_sec }
}

unsafe fn probe(src: &Path) -> Result<(u32, u32, u32, u64)> {
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), None)?;
    let t = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
    let (w, h) = unpack(t.GetUINT64(&MF_MT_FRAME_SIZE)?);
    let (n, d) = t.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).unwrap_or((60, 1));
    let fps = ((n as f64 / d.max(1) as f64).round() as u32).clamp(1, 240);
    let dur = duration_100ns(&reader).unwrap_or(0);
    Ok((w, h, fps, dur))
}

unsafe fn encode(src: &Path, dst: &Path, p: &Plan, dur: u64, progress: &dyn Fn(f32)) -> Result<()> {
    // Decoder side: scale + convert to NV12 inside the source reader.
    let ra = attrs(&[(MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)], &[])?;
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(src.as_os_str()), &ra)?;
    reader.SetStreamSelection(ALL_STREAMS, false)?;
    reader.SetStreamSelection(FIRST_VIDEO, true)?;
    let has_audio = reader.SetStreamSelection(FIRST_AUDIO, true).is_ok();

    let vin = MFCreateMediaType()?;
    vin.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    vin.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
    vin.SetUINT64(&MF_MT_FRAME_SIZE, pack(p.width, p.height))?;
    vin.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    vin.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    // No frame rate here: the reader keeps the source rate and frames are
    // dropped below (every `step`-th kept), so conversions don't stack.
    reader.SetCurrentMediaType(FIRST_VIDEO, None, &vin)?;
    // What the encoder is told it receives: the reduced rate.
    let venc_in = MFCreateMediaType()?;
    vin.CopyAllItems(&venc_in)?;
    venc_in.SetUINT64(&MF_MT_FRAME_RATE, pack(p.fps, 1))?;

    let ain = MFCreateMediaType()?;
    ain.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    ain.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    ain.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    ain.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
    ain.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
    ain.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
    ain.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000)?;
    let has_audio = has_audio && reader.SetCurrentMediaType(FIRST_AUDIO, None, &ain).is_ok();
    if !has_audio {
        let _ = reader.SetStreamSelection(FIRST_AUDIO, false);
    }

    // Encoder side: hardware H.264 + AAC into MP4.
    let wa = attrs(
        &[(MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1), (MF_SINK_WRITER_DISABLE_THROTTLING, 1)],
        &[(MF_TRANSCODE_CONTAINERTYPE, MFTranscodeContainerType_MPEG4)],
    )?;
    let writer = MFCreateSinkWriterFromURL(&HSTRING::from(dst.as_os_str()), None, &wa)?;

    let vout = MFCreateMediaType()?;
    vout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    vout.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    vout.SetUINT32(&MF_MT_AVG_BITRATE, p.video_bps)?;
    vout.SetUINT64(&MF_MT_FRAME_SIZE, pack(p.width, p.height))?;
    vout.SetUINT64(&MF_MT_FRAME_RATE, pack(p.fps, 1))?;
    vout.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    vout.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    vout.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)?;
    let vs = writer.AddStream(&vout)?;
    writer.SetInputMediaType(vs, &venc_in, None)?;

    let mut astream = None;
    if has_audio {
        let aout = MFCreateMediaType()?;
        aout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        aout.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        aout.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        aout.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
        aout.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        aout.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, p.audio_bytes_per_sec)?;
        let a = writer.AddStream(&aout)?;
        writer.SetInputMediaType(a, &ain, None)?;
        astream = Some(a);
    }
    writer.BeginWriting()?;

    let mut kinds: HashMap<u32, bool> = HashMap::new(); // stream -> is_video
    let mut ended = 0;
    let wanted = 1 + astream.is_some() as usize;
    let mut frame_no: u64 = 0;
    let frame_dur = 10_000_000i64 / p.fps.max(1) as i64;
    let mut last_report = 0.0f32;
    while ended < wanted {
        let mut idx = 0u32;
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample = None;
        reader.ReadSample(ANY_STREAM, 0, Some(&mut idx), Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        let is_video = *kinds.entry(idx).or_insert_with(|| {
            reader
                .GetCurrentMediaType(idx)
                .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
                .map(|g| g == MFMediaType_Video)
                .unwrap_or(false)
        });
        if let Some(s) = sample {
            if is_video {
                if frame_no % p.step as u64 == 0 {
                    s.SetSampleDuration(frame_dur)?;
                    writer.WriteSample(vs, &s)?;
                }
                frame_no += 1;
                if dur > 0 {
                    let f = (ts as f64 / dur as f64).clamp(0.0, 1.0) as f32;
                    if f - last_report > 0.02 {
                        last_report = f;
                        progress(f);
                    }
                }
            } else if let Some(a) = astream {
                writer.WriteSample(a, &s)?;
            }
        }
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            ended += 1;
        }
    }
    writer.Finalize()?;
    progress(1.0);
    Ok(())
}
