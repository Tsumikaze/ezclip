//! Audio thread: WASAPI loopback (what you hear) + optional microphone, mixed
//! to 48 kHz stereo and encoded to AAC.
//!
//! The mixer runs on the same clock as the video thread and always emits
//! exactly "elapsed time × 48000" samples, padding with silence when a device
//! has nothing (loopback is silent when nothing plays). This keeps audio and
//! video in sync no matter what the devices do.
use super::buffer::{ClipBuffer, TICKS};
use super::encoder::{AacEncoder, AUDIO_RATE};
use super::mux::AAC_FRAME;
use super::StatusSink;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{Result, PWSTR};
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::*;

const RATE: f64 = AUDIO_RATE as f64;
/// Mixer runs this far behind real time so devices can deliver their data.
const LATENCY: f64 = 0.08;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Loopback,
    Mic,
}

#[derive(Clone, Copy)]
enum SampleFmt {
    F32,
    I16,
    I24,
    I32,
}

struct Device {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    id: String,
    channels: usize,
    rate: u32,
    bits: u16,
    fmt: SampleFmt,
}

/// Microphone choice for the audio thread.
pub struct MicParams {
    /// Device id; None = Windows' default microphone.
    pub device: Option<String>,
    /// Volume in percent, changed live by the recorder on settings reload.
    pub gain: Arc<AtomicU32>,
}

struct Source {
    kind: Kind,
    dev: Option<Device>,
    /// Mic only: the chosen device (None = default) and its live volume.
    wanted: Option<String>,
    gain: Option<Arc<AtomicU32>>,
    /// Why the device isn't working, or a note (e.g. fell back to default).
    problem: Option<String>,
    /// Using the default mic because the chosen one isn't available.
    fell_back: bool,
    /// Any non-zero sample since opening (all zeros = muted or blocked).
    heard: bool,
    opened_at: Instant,
    /// Stereo frames at 48 kHz, waiting to be mixed.
    fifo: VecDeque<[f32; 2]>,
    /// Timeline position (in 48 kHz samples since pipeline start) of fifo[0].
    fifo_pos: Option<u64>,
    // linear resampler state
    prev: [f32; 2],
    frac: f64,
    last_check: Instant,
}

impl Source {
    fn new(kind: Kind, mic: Option<&MicParams>) -> Self {
        let mut s = Self {
            kind,
            dev: None,
            wanted: mic.and_then(|m| m.device.clone()),
            gain: mic.map(|m| m.gain.clone()),
            problem: None,
            fell_back: false,
            heard: false,
            opened_at: Instant::now(),
            fifo: VecDeque::new(),
            fifo_pos: None,
            prev: [0.0; 2],
            frac: 0.0,
            last_check: Instant::now(),
        };
        s.reopen();
        s
    }

    /// Human status for the sidebar.
    fn describe(&self) -> String {
        let name = if self.kind == Kind::Loopback { "System audio" } else { "Microphone" };
        if self.dev.is_none() {
            return format!("{name} ({})", self.problem.as_deref().unwrap_or("unavailable"));
        }
        if self.kind == Kind::Mic && !self.heard && self.opened_at.elapsed() > Duration::from_secs(8) {
            return format!("{name} (silent: muted, or the wrong device?)");
        }
        match &self.problem {
            Some(p) => format!("{name} ({p})"),
            None => name.to_string(),
        }
    }

    fn reopen(&mut self) {
        self.dev = None;
        self.fifo.clear();
        self.fifo_pos = None;
        self.heard = false;
        self.opened_at = Instant::now();
        self.fell_back = false;
        let label = if self.kind == Kind::Loopback { "system" } else { "mic" };
        let mut result = unsafe { open_device(self.kind, self.wanted.as_deref()) };
        if let (Err(e), Some(id)) = (&result, &self.wanted) {
            // The chosen mic is unplugged or gone: use the default until it's back.
            crate::log::write(&format!("audio mic '{id}' unavailable ({e}); using the default mic"));
            result = unsafe { open_device(self.kind, None) };
            self.fell_back = result.is_ok();
        }
        match result {
            Ok(d) => {
                crate::log::write(&format!("audio {label}: {} Hz, {} ch, {} bit, id {}", d.rate, d.channels, d.bits, d.id));
                self.problem = self.fell_back.then(|| "chosen mic not found, using the default".to_string());
                self.dev = Some(d);
            }
            Err(e) => {
                let why = crate::audiodev::explain(&e);
                crate::log::write(&format!("audio {label} open failed: {why} ({e})"));
                self.problem = Some(why);
            }
        }
        self.last_check = Instant::now();
    }

    /// Every few seconds: follow the Windows default device (e.g. user
    /// switched from speakers to headset), retry a chosen mic that was
    /// unplugged, and recover from errors.
    fn maintain(&mut self) {
        if self.last_check.elapsed() < Duration::from_secs(3) {
            return;
        }
        self.last_check = Instant::now();
        if self.wanted.is_some() {
            // A specific mic was chosen: reopen if it failed or we're on the fallback.
            if self.dev.is_none() || self.fell_back {
                self.reopen();
            }
            return;
        }
        let current = unsafe { default_device_id(self.kind).ok() };
        let changed = match (&self.dev, &current) {
            (Some(d), Some(id)) => &d.id != id,
            (None, Some(_)) => true,
            _ => false,
        };
        if changed {
            self.reopen();
        }
    }

    /// Read all pending packets and place them on the shared timeline using
    /// the device's own capture timestamps (QPC), so audio lines up with video
    /// even when a device starts late or goes quiet.
    fn poll(&mut self, clock: &Clock) {
        let Some(dev) = &self.dev else { return };
        let mut packets: Vec<(Option<u64>, Vec<[f32; 2]>)> = Vec::new();
        let mut heard = false;
        let rate = dev.rate;
        let result: Result<()> = unsafe {
            (|| loop {
                let n = dev.capture.GetNextPacketSize()?;
                if n == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                dev.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let ts_ok = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0 && qpc != 0;
                let mut buf = Vec::with_capacity(frames as usize);
                convert(data, frames as usize, dev.channels, dev.fmt, silent, &mut buf);
                if !silent && buf.iter().any(|f| f[0] != 0.0 || f[1] != 0.0) {
                    heard = true;
                }
                dev.capture.ReleaseBuffer(frames)?;
                let pos = if ts_ok { clock.qpc_to_sample(qpc as i64) } else { None };
                packets.push((pos, buf));
            })()
        };
        self.heard |= heard;
        if let Err(e) = result {
            crate::log::write(&format!("audio capture error ({e}), reopening"));
            self.dev = None;
            self.last_check = Instant::now() - Duration::from_secs(2);
        }
        for (pos, frames) in packets {
            let expected = self.fifo_pos.map(|p| p + self.fifo.len() as u64);
            let at = pos.or(expected).unwrap_or_else(|| clock.now_sample().saturating_sub(frames.len() as u64 * AUDIO_RATE as u64 / rate as u64));
            match expected {
                None => {
                    self.fifo_pos = Some(at);
                    self.reset_resampler();
                }
                Some(exp) => {
                    // Re-sync if the device timestamp disagrees by > 20 ms
                    // (device was quiet, or its clock drifted).
                    let tol = AUDIO_RATE as u64 / 50;
                    if at > exp + tol {
                        self.fifo.extend(std::iter::repeat([0.0; 2]).take((at - exp) as usize));
                        self.reset_resampler();
                    } else if at + tol < exp {
                        let drop_n = ((exp - at) as usize).min(self.fifo.len());
                        let keep = self.fifo.len() - drop_n;
                        self.fifo.truncate(keep);
                        self.reset_resampler();
                    }
                }
            }
            self.resample_into_fifo(&frames, rate);
        }
    }

    fn reset_resampler(&mut self) {
        self.frac = 0.0;
    }

    /// Mix this source's samples for timeline range [from, from + out.len()).
    fn mix_into(&mut self, from: u64, out: &mut [[f32; 2]]) {
        let Some(mut pos) = self.fifo_pos else { return };
        // Drop anything older than the range (arrived too late).
        if pos < from {
            let n = ((from - pos) as usize).min(self.fifo.len());
            self.fifo.drain(..n);
            pos += n as u64;
            if self.fifo.is_empty() {
                pos = pos.max(from);
            }
        }
        let offset = (pos - from) as usize;
        let gain = self.gain.as_ref().map(|g| g.load(Ordering::Relaxed) as f32 / 100.0).unwrap_or(1.0);
        if offset < out.len() {
            let n = (out.len() - offset).min(self.fifo.len());
            for (m, v) in out[offset..offset + n].iter_mut().zip(self.fifo.drain(..n)) {
                m[0] += v[0] * gain;
                m[1] += v[1] * gain;
            }
            pos += n as u64;
        }
        self.fifo_pos = Some(pos);
    }

    fn resample_into_fifo(&mut self, input: &[[f32; 2]], rate: u32) {
        if rate == AUDIO_RATE {
            self.fifo.extend(input.iter().copied());
            return;
        }
        let step = rate as f64 / RATE;
        for &f in input {
            while self.frac < 1.0 {
                let t = self.frac as f32;
                self.fifo.push_back([self.prev[0] + (f[0] - self.prev[0]) * t, self.prev[1] + (f[1] - self.prev[1]) * t]);
                self.frac += step;
            }
            self.frac -= 1.0;
            self.prev = f;
        }
    }
}

/// Maps QPC timestamps and "now" onto the pipeline timeline (48 kHz samples).
struct Clock {
    start: Instant,
    /// QPC time of `start`, in 100 ns units.
    start_qpc: i64,
}

impl Clock {
    fn new(start: Instant) -> Self {
        let mut freq = 0i64;
        let mut now = 0i64;
        unsafe {
            let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut freq);
            let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut now);
        }
        let now_100ns = (now as i128 * TICKS as i128 / freq.max(1) as i128) as i64;
        let elapsed = (start.elapsed().as_nanos() / 100) as i64;
        Self { start, start_qpc: now_100ns - elapsed }
    }
    fn qpc_to_sample(&self, qpc_100ns: i64) -> Option<u64> {
        let t = qpc_100ns - self.start_qpc;
        (t >= 0).then(|| (t as i128 * AUDIO_RATE as i128 / TICKS as i128) as u64)
    }
    fn now_sample(&self) -> u64 {
        (self.start.elapsed().as_secs_f64() * RATE) as u64
    }
}

unsafe fn convert(data: *const u8, frames: usize, channels: usize, fmt: SampleFmt, silent: bool, out: &mut Vec<[f32; 2]>) {
    if silent || data.is_null() {
        out.extend(std::iter::repeat([0.0; 2]).take(frames));
        return;
    }
    let read = |i: usize| -> f32 {
        match fmt {
            SampleFmt::F32 => *(data as *const f32).add(i),
            SampleFmt::I16 => *(data as *const i16).add(i) as f32 / 32768.0,
            SampleFmt::I32 => *(data as *const i32).add(i) as f32 / 2147483648.0,
            SampleFmt::I24 => {
                let p = data.add(i * 3);
                let v = (*p as i32) | ((*p.add(1) as i32) << 8) | ((*p.add(2) as i8 as i32) << 16);
                v as f32 / 8388608.0
            }
        }
    };
    for f in 0..frames {
        let base = f * channels;
        let frame = if channels == 1 {
            let v = read(base);
            [v, v]
        } else {
            [read(base), read(base + 1)]
        };
        out.push(frame);
    }
}

unsafe fn default_device_id(kind: Kind) -> Result<String> {
    let enumr: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = enumr.GetDefaultAudioEndpoint(if kind == Kind::Loopback { eRender } else { eCapture }, eConsole)?;
    let id: PWSTR = dev.GetId()?;
    let s = id.to_string().unwrap_or_default();
    CoTaskMemFree(Some(id.0 as _));
    Ok(s)
}

unsafe fn open_device(kind: Kind, mic_id: Option<&str>) -> Result<Device> {
    let dev = if kind == Kind::Mic {
        crate::audiodev::open_capture(mic_id)?
    } else {
        let enumr: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        enumr.GetDefaultAudioEndpoint(eRender, eConsole)?
    };
    let idp: PWSTR = dev.GetId()?;
    let id = idp.to_string().unwrap_or_default();
    CoTaskMemFree(Some(idp.0 as _));
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let pfmt = client.GetMixFormat()?;
    let wf = *pfmt;
    let is_float = match wf.wFormatTag as u32 {
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_EXTENSIBLE => {
            let ext = std::ptr::read_unaligned(pfmt as *const WAVEFORMATEXTENSIBLE);
            let sub = ext.SubFormat;
            sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        }
        _ => false,
    };
    let fmt = match (is_float, wf.wBitsPerSample) {
        (true, _) => SampleFmt::F32,
        (false, 16) => SampleFmt::I16,
        (false, 24) => SampleFmt::I24,
        _ => SampleFmt::I32,
    };
    let flags = if kind == Kind::Loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
    let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, pfmt, None);
    let (channels, rate) = (wf.nChannels as usize, wf.nSamplesPerSec);
    CoTaskMemFree(Some(pfmt as _));
    r?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    Ok(Device { client, capture, id, channels: channels.max(1), rate, bits: wf.wBitsPerSample, fmt })
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

pub fn thread_main(system: bool, mic: Option<MicParams>, start: Instant, stop: Arc<AtomicBool>, buf: Arc<ClipBuffer>, status: StatusSink) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let enc = match AacEncoder::create() {
        Ok(e) => e,
        Err(e) => {
            crate::log::write(&format!("AAC encoder: {e}"));
            status.audio(&format!("No audio: AAC encoder unavailable ({e})"));
            return;
        }
    };
    let mut sources = Vec::new();
    if system {
        sources.push(Source::new(Kind::Loopback, None));
    }
    if let Some(m) = &mic {
        sources.push(Source::new(Kind::Mic, Some(m)));
    }
    let describe = |sources: &Vec<Source>| sources.iter().map(|s| s.describe()).collect::<Vec<_>>().join(" + ");
    let mut described = describe(&sources);
    status.audio(&described);

    let clock = Clock::new(start);
    // Start counting from "now" on the shared clock so we line up with video.
    let mut produced: u64 = (start.elapsed().as_secs_f64() * RATE) as u64;
    let first_sample = produced;
    let mut pending: Vec<i16> = Vec::with_capacity(AAC_FRAME as usize * 8);
    let mut pending_start = produced;
    let mut aac_index: i64 = 0;
    let base_pts = (first_sample as i64 * TICKS) / AUDIO_RATE as i64;
    let mut mix: Vec<[f32; 2]> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        for s in sources.iter_mut() {
            s.maintain();
            s.poll(&clock);
        }
        let d = describe(&sources);
        if d != described {
            status.audio(&d);
            described = d;
        }

        let target = ((start.elapsed().as_secs_f64() - LATENCY).max(0.0) * RATE) as u64;
        if target <= produced {
            continue;
        }
        let need = (target - produced) as usize;
        mix.clear();
        mix.resize(need, [0.0; 2]);
        for s in sources.iter_mut() {
            s.mix_into(produced, &mut mix);
        }
        produced = target;
        for m in &mix {
            pending.push((m[0].clamp(-1.0, 1.0) * 32767.0) as i16);
            pending.push((m[1].clamp(-1.0, 1.0) * 32767.0) as i16);
        }

        // Feed the encoder in ~100 ms chunks.
        if pending.len() >= (AUDIO_RATE as usize / 10) * 2 {
            let pts = (pending_start as i64 * TICKS) / AUDIO_RATE as i64;
            match enc.encode(&pending, pts) {
                Ok(frames) => {
                    for f in frames {
                        let pts = base_pts + (aac_index * AAC_FRAME * TICKS) / AUDIO_RATE as i64;
                        aac_index += 1;
                        buf.push_audio(pts, f);
                    }
                }
                Err(e) => crate::log::write(&format!("AAC encode: {e}")),
            }
            pending_start += (pending.len() / 2) as u64;
            pending.clear();
        }
    }
}
