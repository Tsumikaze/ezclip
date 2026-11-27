//! Video thread: Desktop Duplication → GPU colour conversion → hardware encoder.
//!
//! Everything stays on the GPU: the captured desktop texture is converted to
//! NV12 (and optionally scaled) by the D3D11 video processor — the same fixed
//! function block used for video playback — and handed to the encoder as a
//! texture. The CPU only paces frames and moves small encoded packets around.
use super::buffer::{ClipBuffer, VideoCodec, TICKS};
use super::encoder::{texture_sample, memory_sample, EncoderEvent, VideoEncoder, VideoSettings};
use super::StatusSink;
use crate::config::{Quality, Resolution};
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::*;

pub struct VideoParams {
    pub monitor: u32,
    pub fps: u32,
    pub resolution: Resolution,
    pub quality: Quality,
    pub codec: crate::config::Codec,
}

const POOL: usize = 8;

struct Gpu {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
}

impl Gpu {
    fn new(adapter: &IDXGIAdapter1) -> Result<Self> {
        unsafe {
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let device = device.unwrap();
            let ctx = ctx.unwrap();
            // The encoder uses the device from its own threads.
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            // Ask the GPU scheduler to favour us slightly so capture doesn't
            // stutter when a game pegs the GPU at 100 %.
            if let Ok(dxgi) = device.cast::<IDXGIDevice>() {
                let _ = dxgi.SetGPUThreadPriority(7);
            }
            Ok(Self { vdev: device.cast()?, vctx: ctx.cast()?, device, ctx })
        }
    }
}

/// BGRA desktop → NV12 (scaled) via the D3D11 video processor.
struct Converter {
    src: ID3D11Texture2D,
    src_w: u32,
    src_h: u32,
    src_fmt: DXGI_FORMAT,
    vp: ID3D11VideoProcessor,
    in_view: ID3D11VideoProcessorInputView,
    pool: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    staging: Option<ID3D11Texture2D>,
    out_w: u32,
    out_h: u32,
}

fn tex_desc(w: u32, h: u32, format: DXGI_FORMAT, bind: D3D11_BIND_FLAG) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}

impl Converter {
    fn new(gpu: &Gpu, src_w: u32, src_h: u32, src_fmt: DXGI_FORMAT, out_w: u32, out_h: u32, fps: u32, cpu_readback: bool) -> Result<Self> {
        unsafe {
            let mut src = None;
            gpu.device.CreateTexture2D(
                &tex_desc(src_w, src_h, src_fmt, D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE),
                None,
                Some(&mut src),
            )?;
            let src = src.unwrap();

            let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
                InputWidth: src_w,
                InputHeight: src_h,
                OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let vpe = gpu.vdev.CreateVideoProcessorEnumerator(&content)?;
            let vp = gpu.vdev.CreateVideoProcessor(&vpe, 0)?;

            let in_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 } },
            };
            let mut in_view = None;
            gpu.vdev.CreateVideoProcessorInputView(&src, &vpe, &in_desc, Some(&mut in_view))?;

            let out_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut pool = Vec::with_capacity(POOL);
            for _ in 0..POOL {
                let mut t = None;
                gpu.device.CreateTexture2D(&tex_desc(out_w, out_h, DXGI_FORMAT_NV12, D3D11_BIND_RENDER_TARGET), None, Some(&mut t))?;
                let t = t.unwrap();
                let mut v = None;
                gpu.vdev.CreateVideoProcessorOutputView(&t, &vpe, &out_desc, Some(&mut v))?;
                pool.push((t, v.unwrap()));
            }

            // Full-range RGB in, BT.709 limited-range YUV out (what players expect).
            if let Ok(vctx1) = gpu.vctx.cast::<ID3D11VideoContext1>() {
                vctx1.VideoProcessorSetStreamColorSpace1(&vp, 0, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
                vctx1.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
            }
            let full_src = RECT { left: 0, top: 0, right: src_w as i32, bottom: src_h as i32 };
            let full_dst = RECT { left: 0, top: 0, right: out_w as i32, bottom: out_h as i32 };
            gpu.vctx.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&full_src));
            gpu.vctx.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&full_dst));
            gpu.vctx.VideoProcessorSetOutputTargetRect(&vp, true, Some(&full_dst));
            gpu.vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
            gpu.vctx.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);

            let staging = if cpu_readback {
                let mut d = tex_desc(out_w, out_h, DXGI_FORMAT_NV12, D3D11_BIND_FLAG(0));
                d.Usage = D3D11_USAGE_STAGING;
                d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut t = None;
                gpu.device.CreateTexture2D(&d, None, Some(&mut t))?;
                t
            } else {
                None
            };

            Ok(Self { src, src_w, src_h, src_fmt, vp, in_view: in_view.unwrap(), pool, next: 0, staging, out_w, out_h })
        }
    }

    /// Convert the current desktop copy into the next pool texture.
    fn convert(&mut self, gpu: &Gpu) -> Result<ID3D11Texture2D> {
        let (tex, view) = &self.pool[self.next];
        self.next = (self.next + 1) % self.pool.len();
        unsafe {
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: ManuallyDrop::new(Some(self.in_view.clone())),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let mut streams = [stream];
            let r = gpu.vctx.VideoProcessorBlt(&self.vp, view, 0, &streams);
            ManuallyDrop::drop(&mut streams[0].pInputSurface);
            r?;
        }
        Ok(tex.clone())
    }

    /// Fallback path: copy the NV12 frame to system memory.
    fn read_back(&self, gpu: &Gpu, tex: &ID3D11Texture2D, out: &mut Vec<u8>) -> Result<()> {
        let staging = self.staging.as_ref().unwrap();
        let (w, h) = (self.out_w as usize, self.out_h as usize);
        out.resize(w * h * 3 / 2, 0);
        unsafe {
            gpu.ctx.CopyResource(staging, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let pitch = m.RowPitch as usize;
            let base = m.pData as *const u8;
            for row in 0..(h + h / 2) {
                let src = std::slice::from_raw_parts(base.add(row * pitch), w);
                out[row * w..row * w + w].copy_from_slice(src);
            }
            gpu.ctx.Unmap(staging, 0);
        }
        Ok(())
    }
}

/// Desktop Duplication wrapper that survives mode changes / UAC prompts.
struct Duplicator {
    output: IDXGIOutput1,
    dup: Option<IDXGIOutputDuplication>,
    lost_since: Option<Instant>,
    last_retry: Instant,
}

enum Grab {
    None,
    Frame(ID3D11Texture2D),
}

impl Duplicator {
    fn new(output: IDXGIOutput1, device: &ID3D11Device) -> Result<Self> {
        let dup = unsafe { output.DuplicateOutput(device)? };
        Ok(Self { output, dup: Some(dup), lost_since: None, last_retry: Instant::now() })
    }

    fn grab(&mut self, device: &ID3D11Device) -> std::result::Result<Grab, String> {
        if self.dup.is_none() {
            // Duplication was lost (resolution change, fullscreen switch, UAC
            // secure desktop...). Retry a few times per second.
            if self.last_retry.elapsed() < Duration::from_millis(250) {
                return Ok(Grab::None);
            }
            self.last_retry = Instant::now();
            match unsafe { self.output.DuplicateOutput(device) } {
                Ok(d) => {
                    self.dup = Some(d);
                    self.lost_since = None;
                }
                Err(e) => {
                    let since = *self.lost_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(10) {
                        return Err(format!("screen capture unavailable: {e}"));
                    }
                    return Ok(Grab::None);
                }
            }
        }
        let dup = self.dup.as_ref().unwrap();
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match dup.AcquireNextFrame(0, &mut info, &mut res) {
                Ok(()) => {
                    let tex = res.and_then(|r| r.cast::<ID3D11Texture2D>().ok());
                    match tex {
                        // Caller copies, then we release in `release()`.
                        Some(t) => Ok(Grab::Frame(t)),
                        None => {
                            let _ = dup.ReleaseFrame();
                            Ok(Grab::None)
                        }
                    }
                }
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => Ok(Grab::None),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST || e.code() == DXGI_ERROR_INVALID_CALL => {
                    self.dup = None;
                    self.lost_since.get_or_insert_with(Instant::now);
                    Ok(Grab::None)
                }
                Err(e) => {
                    self.dup = None;
                    crate::log::write(&format!("AcquireNextFrame: {e}"));
                    Ok(Grab::None)
                }
            }
        }
    }

    fn release(&self) {
        if let Some(d) = &self.dup {
            unsafe {
                let _ = d.ReleaseFrame();
            }
        }
    }
}

/// High-resolution frame pacer on a shared clock with the audio thread.
struct Pacer {
    start: Instant,
    interval: f64, // in 100 ns ticks
    next: u64,
    timer: HANDLE,
}

impl Pacer {
    fn new(start: Instant, fps: u32) -> Self {
        let timer = unsafe {
            CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0).unwrap_or_default()
        };
        let mut p = Self { start, interval: TICKS as f64 / fps as f64, next: 0, timer };
        p.next = p.now_index();
        p
    }

    fn now_ticks(&self) -> f64 {
        self.start.elapsed().as_nanos() as f64 / 100.0
    }

    fn now_index(&self) -> u64 {
        (self.now_ticks() / self.interval).ceil() as u64
    }

    /// Sleep until the next frame slot. Returns its timestamp.
    fn wait(&mut self, stop: &AtomicBool) -> Option<i64> {
        // If we fell behind (e.g. system hiccup), skip slots instead of bursting.
        let now_idx = self.now_index();
        if now_idx > self.next + 2 {
            self.next = now_idx;
        }
        let target = self.next as f64 * self.interval;
        loop {
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let remaining = target - self.now_ticks();
            if remaining <= 0.0 {
                break;
            }
            let chunk = remaining.min(1_000_000.0); // ≤100 ms so we notice stop
            if !self.timer.is_invalid() {
                unsafe {
                    let due = -(chunk as i64).max(1);
                    if SetWaitableTimer(self.timer, &due, 0, None, None, false).is_ok() {
                        WaitForSingleObject(self.timer, INFINITE);
                        continue;
                    }
                }
            }
            std::thread::sleep(Duration::from_nanos(chunk as u64 * 100));
        }
        let pts = target as i64;
        self.next += 1;
        Some(pts)
    }
}

impl Drop for Pacer {
    fn drop(&mut self) {
        if !self.timer.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.timer);
            }
        }
    }
}

pub fn thread_main(p: VideoParams, start: Instant, stop: Arc<AtomicBool>, buf: Arc<ClipBuffer>, status: StatusSink) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    while !stop.load(Ordering::Relaxed) {
        match run(&p, start, &stop, &buf, &status) {
            Ok(()) => break,
            Err(e) => {
                crate::log::write(&format!("video: {e}"));
                status.video_error(&e);
                buf.clear_video();
                // Try again shortly (monitor unplugged, driver reset, lock screen...).
                for _ in 0..30 {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

fn e(what: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |err| format!("{what}: {err}")
}

fn run(p: &VideoParams, start: Instant, stop: &AtomicBool, buf: &ClipBuffer, status: &StatusSink) -> std::result::Result<(), String> {
    let mons = crate::display::monitors();
    let m = mons.get(p.monitor as usize).or(mons.first()).ok_or("no monitor found")?;
    let (dw, dh) = (m.width, m.height);
    let (ow, oh) = p.resolution.output_size(dw, dh);
    let gpu = Gpu::new(&m.adapter).map_err(e("create D3D11 device"))?;
    let luid = unsafe { m.adapter.GetDesc1().map_err(e("adapter"))?.AdapterLuid };
    let settings = |codec: crate::config::Codec| {
        let bitrate = (p.quality.bitrate(ow, oh, p.fps) as f64 * codec.bitrate_factor()) as u32;
        VideoSettings { width: ow, height: oh, fps: p.fps, bitrate, codec }
    };
    // HEVC/AV1 need a hardware encoder; if there isn't one, record H.264.
    let mut note = String::new();
    let (enc, vs) = match VideoEncoder::create(&gpu.device, luid, &settings(p.codec)) {
        Ok(enc) => (enc, settings(p.codec)),
        Err(err) if p.codec != crate::config::Codec::H264 => {
            crate::log::write(&format!("{} encoder unavailable ({err}); using H.264", p.codec.label()));
            note = format!(" ({} not available, using H.264)", p.codec.label());
            let vs = settings(crate::config::Codec::H264);
            (VideoEncoder::create(&gpu.device, luid, &vs)?, vs)
        }
        Err(err) => return Err(err),
    };
    let bitrate = vs.bitrate;
    crate::log::write(&format!(
        "video: {dw}x{dh} -> {ow}x{oh} @ {} fps, {} {} kbps, encoder '{}' (gpu input: {}, async: {})",
        p.fps,
        vs.codec.label(),
        bitrate / 1000,
        enc.name,
        enc.gpu_input,
        enc.is_async()
    ));

    let mut dup = Duplicator::new(m.output.clone(), &gpu.device).map_err(e("start screen capture"))?;
    let mut conv = Converter::new(&gpu, dw, dh, DXGI_FORMAT_B8G8R8A8_UNORM, ow, oh, p.fps, !enc.gpu_input).map_err(e("video processor"))?;

    let codec = match vs.codec {
        crate::config::Codec::H264 => VideoCodec::H264,
        crate::config::Codec::Hevc => VideoCodec::Hevc,
        crate::config::Codec::Av1 => VideoCodec::Av1,
    };
    buf.set_video_format(ow, oh, p.fps, codec);
    status.video_ok(&format!(
        "{ow}×{oh} · {} fps · {} Mbps · {} {}{note}",
        p.fps,
        (bitrate + 500_000) / 1_000_000,
        vs.codec.label(),
        if enc.hardware { "GPU encoder" } else { "CPU encoder" }
    ));

    let mut pacer = Pacer::new(start, p.fps);
    let frame_dur = TICKS / p.fps as i64;
    let mut cpu_frame = Vec::new();

    // Capture one frame, convert it, wrap it as an encoder input sample.
    let mut make_input = |pacer: &mut Pacer, conv: &mut Converter| -> std::result::Result<Option<windows::Win32::Media::MediaFoundation::IMFSample>, String> {
        let Some(pts) = pacer.wait(stop) else { return Ok(None) };
        if let Grab::Frame(t) = dup.grab(&gpu.device)? {
            let mut d = D3D11_TEXTURE2D_DESC::default();
            unsafe { t.GetDesc(&mut d) };
            if d.Width != conv.src_w || d.Height != conv.src_h || d.Format != conv.src_fmt {
                // Desktop resolution changed: rebuild the converter, keep the
                // encoder (the video processor scales to the fixed output size).
                crate::log::write(&format!("desktop changed to {}x{} (format {})", d.Width, d.Height, d.Format.0));
                *conv = Converter::new(&gpu, d.Width, d.Height, d.Format, ow, oh, p.fps, !enc.gpu_input).map_err(e("video processor"))?;
            }
            unsafe { gpu.ctx.CopyResource(&conv.src, &t) };
            dup.release();
        }
        let tex = conv.convert(&gpu).map_err(e("convert frame"))?;
        let sample = if enc.gpu_input {
            texture_sample(&tex, pts, frame_dur)
        } else {
            conv.read_back(&gpu, &tex, &mut cpu_frame).map_err(e("read back frame"))?;
            memory_sample(&cpu_frame, pts, frame_dur)
        }
        .map_err(e("create sample"))?;
        Ok(Some(sample))
    };

    let mut last_mem_log = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        if enc.is_async() {
            match enc.next_event().map_err(e("encoder event"))? {
                EncoderEvent::NeedInput => match make_input(&mut pacer, &mut conv)? {
                    Some(s) => enc.input(&s).map_err(e("encode"))?,
                    None => return Ok(()),
                },
                EncoderEvent::HaveOutput => {
                    if let Some((pts, data, key)) = enc.output().map_err(e("encoder output"))? {
                        buf.push_video(pts, data, key);
                    }
                }
                EncoderEvent::Other => {}
            }
        } else {
            match make_input(&mut pacer, &mut conv)? {
                Some(s) => enc.input(&s).map_err(e("encode"))?,
                None => return Ok(()),
            }
            while let Some((pts, data, key)) = enc.output().map_err(e("encoder output"))? {
                buf.push_video(pts, data, key);
            }
        }
        if last_mem_log.elapsed() > Duration::from_secs(60) {
            last_mem_log = Instant::now();
            crate::log::write(&format!("buffer holds {:.1} MB", buf.memory_bytes() as f64 / 1e6));
        }
    }
}
