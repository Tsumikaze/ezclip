//! Live microphone level for Settings: captures from the chosen device in
//! the window process (only while the Audio settings are on screen), so you
//! can see the mic works before recording anything.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::CoTaskMemFree;

pub struct MicMeter {
    /// The device this meter listens to (None = Windows default).
    pub device: Option<String>,
    stop: Arc<AtomicBool>,
    /// Peak level 0..1 (f32 bits), with a short fall-off.
    level: Arc<AtomicU32>,
    error: Arc<Mutex<Option<String>>>,
}

impl MicMeter {
    pub fn start(device: Option<String>, ctx: eframe::egui::Context) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let error = Arc::new(Mutex::new(None));
        {
            let (stop, level, error, device) = (stop.clone(), level.clone(), error.clone(), device.clone());
            std::thread::Builder::new()
                .name("mic-meter".into())
                .spawn(move || {
                    if let Err(e) = unsafe { run(device.as_deref(), &stop, &level, &ctx) } {
                        *error.lock().unwrap() = Some(crate::audiodev::explain(&e));
                        ctx.request_repaint();
                    }
                })
                .unwrap();
        }
        Self { device, stop, level, error }
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for MicMeter {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

unsafe fn run(device: Option<&str>, stop: &AtomicBool, level: &AtomicU32, ctx: &eframe::egui::Context) -> windows::core::Result<()> {
    let dev = crate::audiodev::open_capture(device)?;
    let client: IAudioClient = dev.Activate(windows::Win32::System::Com::CLSCTX_ALL, None)?;
    let pfmt = client.GetMixFormat()?;
    let wf = *pfmt;
    let float = match wf.wFormatTag as u32 {
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_EXTENSIBLE => {
            let sub = std::ptr::read_unaligned(pfmt as *const WAVEFORMATEXTENSIBLE).SubFormat;
            sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        }
        _ => false,
    };
    let bytes = (wf.wBitsPerSample / 8).max(1) as usize;
    let channels = wf.nChannels.max(1) as usize;
    let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 2_000_000, 0, pfmt, None);
    CoTaskMemFree(Some(pfmt as _));
    r?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    let mut shown = 0f32;
    while !stop.load(Ordering::Relaxed) {
        let mut peak = 0f32;
        loop {
            let n = capture.GetNextPacketSize()?;
            if n == 0 {
                break;
            }
            let mut data = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 && !data.is_null() {
                let count = frames as usize * channels;
                for i in 0..count {
                    let v = if float {
                        *(data as *const f32).add(i)
                    } else {
                        match bytes {
                            2 => *(data as *const i16).add(i) as f32 / 32768.0,
                            3 => {
                                let p = data.add(i * 3);
                                ((*p as i32) | ((*p.add(1) as i32) << 8) | ((*p.add(2) as i8 as i32) << 16)) as f32 / 8388608.0
                            }
                            _ => *(data as *const i32).add(i) as f32 / 2147483648.0,
                        }
                    };
                    peak = peak.max(v.abs());
                }
            }
            capture.ReleaseBuffer(frames)?;
        }
        // Fast attack, gentle fall-off, like a normal level meter.
        shown = peak.max(shown * 0.85);
        level.store(shown.to_bits(), Ordering::Relaxed);
        ctx.request_repaint();
        std::thread::sleep(Duration::from_millis(33));
    }
    let _ = client.Stop();
    Ok(())
}
