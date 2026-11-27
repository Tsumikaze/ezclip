//! Playback inside the trim editor: video frames decoded by Media Foundation
//! and shown in the editor, sound played through the default speakers.
//!
//! Two threads share one start time: the video thread shows each frame when
//! it is due (dropping frames if decoding falls behind), the audio thread
//! keeps the WASAPI buffer filled. Both stop at `until` or when dropped.
use super::media::{attrs, mf_thread_init, pack, unpack, ALL_STREAMS, FIRST_AUDIO, FIRST_VIDEO};
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{Result, GUID, HSTRING, PROPVARIANT};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

pub enum PlayerEvent {
    /// A frame to show, and its time in seconds.
    Frame(f64, egui::ColorImage),
    /// Reached `until` or the end of the clip.
    Ended,
}

pub struct Player {
    stop: Arc<AtomicBool>,
    rx: Receiver<PlayerEvent>,
}

impl Player {
    /// Play `path` from `from` seconds, stopping at `until` (None = end of clip).
    pub fn start(ctx: egui::Context, path: PathBuf, from: f64, until: Option<f64>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        // Both threads count from the same moment so picture and sound line up.
        let t0 = Instant::now() + Duration::from_millis(120);
        {
            let (stop, path, ctx) = (stop.clone(), path.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("play-video".into())
                .spawn(move || {
                    mf_thread_init();
                    if let Err(e) = unsafe { video_loop(&path, from, until, t0, &stop, &tx, &ctx) } {
                        crate::log::write(&format!("playback video: {e}"));
                    }
                    let _ = tx.send(PlayerEvent::Ended);
                    ctx.request_repaint();
                })
                .unwrap();
        }
        {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("play-audio".into())
                .spawn(move || {
                    mf_thread_init();
                    // A clip without sound (or no speakers) just plays silently.
                    if let Err(e) = unsafe { audio_loop(&path, from, until, t0, &stop) } {
                        crate::log::write(&format!("playback audio: {e}"));
                    }
                })
                .unwrap();
        }
        Self { stop, rx }
    }

    pub fn poll(&self) -> Vec<PlayerEvent> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Sleep until `when`, waking early if playback is stopped.
fn sleep_until(when: Instant, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if now >= when {
            return true;
        }
        std::thread::sleep((when - now).min(Duration::from_millis(15)));
    }
}

fn seek(reader: &IMFSourceReader, seconds: f64) -> Result<()> {
    unsafe { reader.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from((seconds.max(0.0) * 1e7) as i64)) }
}

unsafe fn video_loop(
    path: &Path,
    from: f64,
    until: Option<f64>,
    t0: Instant,
    stop: &AtomicBool,
    tx: &Sender<PlayerEvent>,
    ctx: &egui::Context,
) -> Result<()> {
    // Let Media Foundation decode, convert to RGB32 and scale (to ≤ 960×540).
    let a = attrs(&[(MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)], &[])?;
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &a)?;
    reader.SetStreamSelection(ALL_STREAMS, false)?;
    reader.SetStreamSelection(FIRST_VIDEO, true)?;
    let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?;
    let (sw, sh) = unpack(native.GetUINT64(&MF_MT_FRAME_SIZE)?);
    let scale = (sw as f64 / 960.0).max(sh as f64 / 540.0).max(1.0);
    let (w, h) = (((sw as f64 / scale) as u32) & !1, ((sh as f64 / scale) as u32) & !1);
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
    reader.SetCurrentMediaType(FIRST_VIDEO, None, &t)?;
    let (fn_, fd) = native.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).unwrap_or((60, 1));
    let frame = fd.max(1) as f64 / fn_.max(1) as f64;
    seek(&reader, from)?;

    while !stop.load(Ordering::Relaxed) {
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample = None;
        reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(sample) = sample else { continue };
        let t = ts as f64 / 1e7;
        if t + frame / 2.0 < from {
            continue; // decoding up from the keyframe before `from`
        }
        if until.is_some_and(|u| t >= u) {
            break;
        }
        let due = t0 + Duration::from_secs_f64((t - from).max(0.0));
        if Instant::now() > due + Duration::from_secs_f64(frame * 2.0) {
            continue; // running late: drop this frame
        }
        let image = to_image(&sample, w as usize, h as usize)?;
        if !sleep_until(due, stop) {
            break;
        }
        if tx.send(PlayerEvent::Frame(t, image)).is_err() {
            break;
        }
        ctx.request_repaint();
    }
    Ok(())
}

unsafe fn to_image(sample: &IMFSample, w: usize, h: usize) -> Result<egui::ColorImage> {
    use windows::core::Interface;
    let buf = sample.ConvertToContiguousBuffer()?;
    let mut px = vec![0u8; w * h * 4];
    let mut scan0 = std::ptr::null_mut();
    let mut pitch = (w * 4) as i32;
    let two_d = buf.cast::<IMF2DBuffer>().ok();
    let locked_2d = two_d.as_ref().is_some_and(|b| b.Lock2D(&mut scan0, &mut pitch).is_ok());
    if !locked_2d {
        let mut len = 0u32;
        buf.Lock(&mut scan0, None, Some(&mut len))?;
        if (len as usize) < w * h * 4 {
            let _ = buf.Unlock();
            return Err(windows::Win32::Foundation::E_FAIL.into());
        }
    }
    for y in 0..h {
        let row = scan0.offset(y as isize * pitch as isize);
        let src = std::slice::from_raw_parts(row, w * 4);
        let dst = &mut px[y * w * 4..(y + 1) * w * 4];
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = 255;
        }
    }
    if locked_2d {
        let _ = two_d.unwrap().Unlock2D();
    } else {
        let _ = buf.Unlock();
    }
    Ok(egui::ColorImage::from_rgba_unmultiplied([w, h], &px))
}

const RATE: u32 = 48_000;

unsafe fn audio_loop(path: &Path, from: f64, until: Option<f64>, t0: Instant, stop: &AtomicBool) -> Result<()> {
    // Decoder side: 48 kHz stereo float, whatever the clip contains.
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
    reader.SetStreamSelection(ALL_STREAMS, false)?;
    reader.SetStreamSelection(FIRST_AUDIO, true)?;
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
    t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
    t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
    t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 32)?;
    t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 8)?;
    t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 8)?;
    reader.SetCurrentMediaType(FIRST_AUDIO, None, &t)?;
    seek(&reader, from)?;

    // Speaker side: WASAPI converts our format to the device's.
    let enumr: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = enumr.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let wfx = WAVEFORMATEX {
        wFormatTag: 3, // IEEE float
        nChannels: 2,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * 8,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        2_000_000,
        0,
        &wfx,
        None,
    )?;
    let render: IAudioRenderClient = client.GetService()?;
    let capacity = client.GetBufferSize()?;

    let until_frame = until.map(|u| ((u - from).max(0.0) * RATE as f64) as u64);
    let mut written: u64 = 0;
    let mut pending: Vec<f32> = Vec::new(); // interleaved stereo
    let mut ended = false;
    let mut started = false;

    while !stop.load(Ordering::Relaxed) {
        // Keep ~a buffer's worth of decoded audio ready.
        while !ended && pending.len() < (capacity as usize) * 2 {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            reader.ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                ended = true;
                break;
            }
            let Some(s) = sample else { continue };
            let buf = s.ConvertToContiguousBuffer()?;
            let mut p = std::ptr::null_mut();
            let mut len = 0u32;
            buf.Lock(&mut p, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(p as *const f32, len as usize / 4);
            // Skip what lies before `from` (the seek lands a little early).
            let start_t = ts as f64 / 1e7;
            let skip = (((from - start_t) * RATE as f64).max(0.0) as usize * 2).min(data.len());
            pending.extend_from_slice(&data[skip..]);
            let _ = buf.Unlock();
        }
        let padding = client.GetCurrentPadding()?;
        let mut n = (capacity - padding) as usize;
        n = n.min(pending.len() / 2);
        if let Some(limit) = until_frame {
            n = n.min(limit.saturating_sub(written) as usize);
        }
        if n > 0 {
            let dst = render.GetBuffer(n as u32)?;
            std::ptr::copy_nonoverlapping(pending.as_ptr() as *const u8, dst, n * 8);
            render.ReleaseBuffer(n as u32, 0)?;
            pending.drain(..n * 2);
            written += n as u64;
        }
        if !started {
            // Start the sound together with the first video frame.
            if !sleep_until(t0, stop) {
                break;
            }
            client.Start()?;
            started = true;
        }
        let done = (ended && pending.is_empty()) || until_frame.is_some_and(|l| written >= l);
        if done {
            // Let what's buffered play out, then stop.
            while !stop.load(Ordering::Relaxed) && client.GetCurrentPadding().unwrap_or(0) > 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = client.Stop();
    Ok(())
}
