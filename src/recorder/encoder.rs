//! Media Foundation encoders.
//!
//! * Video: the GPU's hardware H.264 encoder (AMD/NVIDIA/Intel all ship one as
//!   an async MFT). NV12 textures are handed over directly on the GPU, so no
//!   frame ever touches system RAM. Falls back to Microsoft's software encoder
//!   only if no hardware encoder can be opened.
//! * Audio: Microsoft's AAC encoder (tiny CPU cost at 48 kHz stereo).
use std::mem::ManuallyDrop;
use windows::core::{Interface, Result, GUID, VARIANT};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

pub const MF_VERSION: u32 = 0x0002_0070;

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

fn enum_mfts(category: GUID, flags: MFT_ENUM_FLAG, input: GUID, output: GUID, major: GUID) -> Vec<IMFActivate> {
    unsafe {
        let inp = MFT_REGISTER_TYPE_INFO { guidMajorType: major, guidSubtype: input };
        let out = MFT_REGISTER_TYPE_INFO { guidMajorType: major, guidSubtype: output };
        let mut acts: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        if MFTEnumEx(category, flags, Some(&inp), Some(&out), &mut acts, &mut count).is_err() || acts.is_null() {
            return Vec::new();
        }
        let list = std::slice::from_raw_parts_mut(acts, count as usize).iter_mut().filter_map(|a| a.take()).collect();
        CoTaskMemFree(Some(acts as _));
        list
    }
}

fn friendly_name(act: &IMFActivate) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut len = 0u32;
        if act.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len)).is_ok() {
            return String::from_utf16_lossy(&buf[..len as usize]);
        }
    }
    "H.264 encoder".into()
}

/// Read the whole payload out of an MF sample.
fn sample_bytes(sample: &IMFSample) -> Result<Vec<u8>> {
    unsafe {
        let buf = sample.ConvertToContiguousBuffer()?;
        let mut ptr = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut ptr, None, Some(&mut len))?;
        let v = std::slice::from_raw_parts(ptr, len as usize).to_vec();
        let _ = buf.Unlock();
        Ok(v)
    }
}

/// Run ProcessOutput once. `Ok(None)` means "needs more input".
fn process_output(mft: &IMFTransform, provides: bool, size: u32) -> Result<Option<IMFSample>> {
    unsafe {
        let sample = if provides {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(size.max(1 << 16))?)?;
            Some(s)
        };
        let mut out = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(sample),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        let r = mft.ProcessOutput(0, &mut out, &mut status);
        let sample = ManuallyDrop::take(&mut out[0].pSample);
        drop(ManuallyDrop::take(&mut out[0].pEvents));
        match r {
            Ok(()) => Ok(sample),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(None),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                // Encoder changed its output format: accept whatever it offers.
                let t = mft.GetOutputAvailableType(0, 0)?;
                mft.SetOutputType(0, &t, 0)?;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

// ================================================================= video ===

pub struct VideoEncoder {
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    provides_samples: bool,
    out_size: u32,
    /// True when the encoder takes D3D11 textures directly.
    pub gpu_input: bool,
    pub name: String,
    pub hardware: bool,
    _manager: Option<IMFDXGIDeviceManager>,
}

pub enum EncoderEvent {
    NeedInput,
    HaveOutput,
    Other,
}

pub struct VideoSettings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    pub codec: crate::config::Codec,
}

impl VideoSettings {
    fn subtype(&self) -> GUID {
        match self.codec {
            crate::config::Codec::H264 => MFVideoFormat_H264,
            crate::config::Codec::Hevc => MFVideoFormat_HEVC,
            crate::config::Codec::Av1 => MFVideoFormat_AV1,
        }
    }
    fn profile(&self) -> u32 {
        match self.codec {
            crate::config::Codec::H264 => eAVEncH264VProfile_High.0 as u32,
            crate::config::Codec::Hevc => eAVEncH265VProfile_Main_420_8.0 as u32,
            crate::config::Codec::Av1 => eAVEncAV1VProfile_Main_420_8.0 as u32,
        }
    }
}

impl VideoEncoder {
    /// Opens a hardware encoder for `s.codec`. Only H.264 falls back to the
    /// software encoder; for HEVC/AV1 the caller falls back to H.264.
    pub fn create(device: &ID3D11Device, adapter_luid: LUID, s: &VideoSettings) -> std::result::Result<Self, String> {
        let mut acts = enum_mfts(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            MFVideoFormat_NV12,
            s.subtype(),
            MFMediaType_Video,
        );
        // Prefer the encoder that lives on the same GPU as the captured monitor.
        acts.sort_by_key(|a| unsafe {
            let luid = a.GetUINT64(&MFT_ENUM_ADAPTER_LUID).unwrap_or(0);
            let want = ((adapter_luid.HighPart as u32 as u64) << 32) | adapter_luid.LowPart as u64;
            luid != want
        });
        let mut errors = Vec::new();
        for act in &acts {
            let name = friendly_name(act);
            match unsafe { Self::open(act, Some(device), s, true) } {
                Ok(mut e) => {
                    e.name = name;
                    return Ok(e);
                }
                Err(err) => {
                    crate::log::write(&format!("hardware encoder '{name}' failed: {err}"));
                    errors.push(format!("{name}: {err}"));
                    unsafe {
                        let _ = act.ShutdownObject();
                    }
                }
            }
        }
        if s.codec != crate::config::Codec::H264 {
            return Err(if errors.is_empty() {
                format!("no {} hardware encoder on this PC", s.codec.label())
            } else {
                errors.join("; ")
            });
        }
        // Software fallback (uses CPU — only if the GPU encoder is unusable).
        let sw = enum_mfts(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            MFVideoFormat_NV12,
            MFVideoFormat_H264,
            MFMediaType_Video,
        );
        for act in &sw {
            match unsafe { Self::open(act, None, s, false) } {
                Ok(mut e) => {
                    e.name = "Software H.264 (no GPU encoder found)".into();
                    return Ok(e);
                }
                Err(err) => errors.push(format!("software: {err}")),
            }
        }
        Err(if errors.is_empty() { "no H.264 encoder available".into() } else { errors.join("; ") })
    }

    unsafe fn open(act: &IMFActivate, device: Option<&ID3D11Device>, s: &VideoSettings, hardware: bool) -> Result<Self> {
        let mft: IMFTransform = act.ActivateObject()?;
        let attrs = mft.GetAttributes()?;
        let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        if is_async {
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        }
        let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);

        let mut manager = None;
        if let Some(dev) = device {
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0 {
                let mut token = 0u32;
                let mut m: Option<IMFDXGIDeviceManager> = None;
                MFCreateDXGIDeviceManager(&mut token, &mut m)?;
                let m = m.unwrap();
                m.ResetDevice(dev, token)?;
                if mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize).is_ok() {
                    manager = Some(m);
                }
            }
        }

        apply_codec_api(&mft, s);

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &s.subtype())?;
        out.SetUINT32(&MF_MT_AVG_BITRATE, s.bitrate)?;
        out.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height))?;
        out.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps, 1))?;
        out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        let _ = out.SetUINT32(&MF_MT_MPEG2_PROFILE, s.profile());
        out.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        out.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
        out.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32)?;
        out.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_709.0 as u32)?;
        mft.SetOutputType(0, &out, 0)?;

        let inp = MFCreateMediaType()?;
        inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height))?;
        inp.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps, 1))?;
        inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
        inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        inp.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
        mft.SetInputType(0, &inp, 0)?;
        // Some encoders reset rate control when types are set; apply again.
        apply_codec_api(&mft, s);

        let info = mft.GetOutputStreamInfo(0)?;
        let provides = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32 != 0;

        mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

        let events = if is_async { Some(mft.cast::<IMFMediaEventGenerator>()?) } else { None };
        Ok(Self {
            mft,
            events,
            provides_samples: provides,
            out_size: info.cbSize.max(s.width * s.height * 2),
            gpu_input: manager.is_some(),
            name: String::new(),
            hardware,
            _manager: manager,
        })
    }

    pub fn is_async(&self) -> bool {
        self.events.is_some()
    }

    /// Async encoders only: block until the encoder wants input or has output.
    pub fn next_event(&self) -> Result<EncoderEvent> {
        let ev = unsafe { self.events.as_ref().unwrap().GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))? };
        let t = unsafe { ev.GetType()? };
        Ok(if t == METransformNeedInput.0 as u32 {
            EncoderEvent::NeedInput
        } else if t == METransformHaveOutput.0 as u32 {
            EncoderEvent::HaveOutput
        } else {
            EncoderEvent::Other
        })
    }

    pub fn input(&self, sample: &IMFSample) -> Result<()> {
        unsafe { self.mft.ProcessInput(0, sample, 0) }
    }

    /// Returns (pts in 100 ns, bitstream bytes, encoder says keyframe) or None.
    /// H.264/HEVC come as Annex-B, AV1 as a low-overhead OBU stream.
    pub fn output(&self) -> Result<Option<(i64, Vec<u8>, bool)>> {
        match process_output(&self.mft, self.provides_samples, self.out_size)? {
            Some(s) => unsafe {
                let pts = s.GetSampleTime().unwrap_or(0);
                let key = s.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
                Ok(Some((pts, sample_bytes(&s)?, key)))
            },
            None => Ok(None),
        }
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            if let Ok(sd) = self.mft.cast::<IMFShutdown>() {
                let _ = sd.Shutdown();
            }
        }
    }
}

/// Rate control / GOP / latency settings (best effort: unsupported ones are skipped).
unsafe fn apply_codec_api(mft: &IMFTransform, s: &VideoSettings) {
    let Ok(api) = mft.cast::<ICodecAPI>() else { return };
    let set = |g: &GUID, v: VARIANT| {
        let _ = api.SetValue(g, &v);
    };
    if api.SetValue(&CODECAPI_AVEncCommonRateControlMode, &VARIANT::from(eAVEncCommonRateControlMode_PeakConstrainedVBR.0 as u32)).is_err() {
        set(&CODECAPI_AVEncCommonRateControlMode, VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32));
    }
    set(&CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(s.bitrate));
    set(&CODECAPI_AVEncCommonMaxBitRate, VARIANT::from(s.bitrate.saturating_mul(3) / 2));
    // 1-second GOP: clips can start within a second of the requested length.
    set(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(s.fps));
    // No B-frames → decode order == display order, simple muxing.
    set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32));
    set(&CODECAPI_AVLowLatencyMode, VARIANT::from(true));
}

/// Wrap a GPU texture as an MF sample (zero copy).
pub fn texture_sample(tex: &ID3D11Texture2D, pts: i64, dur: i64) -> Result<IMFSample> {
    unsafe {
        let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false)?;
        if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
            buf.SetCurrentLength(b2.GetContiguousLength()?)?;
        }
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(pts)?;
        s.SetSampleDuration(dur)?;
        Ok(s)
    }
}

/// System-memory sample (fallback path for encoders that can't take textures).
pub fn memory_sample(bytes: &[u8], pts: i64, dur: i64) -> Result<IMFSample> {
    unsafe {
        let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut ptr = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        buf.Unlock()?;
        buf.SetCurrentLength(bytes.len() as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(pts)?;
        s.SetSampleDuration(dur)?;
        Ok(s)
    }
}

// ================================================================= audio ===

pub const AUDIO_RATE: u32 = 48_000;

pub struct AacEncoder {
    mft: IMFTransform,
    provides_samples: bool,
    out_size: u32,
}

impl AacEncoder {
    pub fn create() -> std::result::Result<Self, String> {
        let acts = enum_mfts(
            MFT_CATEGORY_AUDIO_ENCODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER,
            MFAudioFormat_PCM,
            MFAudioFormat_AAC,
            MFMediaType_Audio,
        );
        let mut last = String::from("no AAC encoder installed");
        for act in &acts {
            match unsafe { Self::open(act) } {
                Ok(e) => return Ok(e),
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    }

    unsafe fn open(act: &IMFActivate) -> Result<Self> {
        let mft: IMFTransform = act.ActivateObject()?;
        let inp = MFCreateMediaType()?;
        inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
        inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * 4)?;

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)?; // 192 kbps
        out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?; // raw AAC frames
        out.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;

        // The AAC encoder wants input first; try the other order just in case.
        if mft.SetInputType(0, &inp, 0).is_ok() {
            mft.SetOutputType(0, &out, 0)?;
        } else {
            mft.SetOutputType(0, &out, 0)?;
            mft.SetInputType(0, &inp, 0)?;
        }
        let info = mft.GetOutputStreamInfo(0)?;
        let provides = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32 != 0;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        Ok(Self { mft, provides_samples: provides, out_size: info.cbSize.max(8192) })
    }

    /// Feed interleaved stereo i16 PCM; returns finished AAC frames.
    pub fn encode(&self, pcm: &[i16], pts: i64) -> Result<Vec<Vec<u8>>> {
        let bytes = unsafe { std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2) };
        let dur = (pcm.len() as i64 / 2) * 10_000_000 / AUDIO_RATE as i64;
        let s = memory_sample(bytes, pts, dur)?;
        let mut frames = Vec::new();
        for attempt in 0..2 {
            match unsafe { self.mft.ProcessInput(0, &s, 0) } {
                Ok(()) => break,
                Err(e) if e.code() == MF_E_NOTACCEPTING && attempt == 0 => {
                    // Encoder is full: drain it, then retry.
                    while let Some(out) = process_output(&self.mft, self.provides_samples, self.out_size)? {
                        frames.push(sample_bytes(&out)?);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        while let Some(out) = process_output(&self.mft, self.provides_samples, self.out_size)? {
            frames.push(sample_bytes(&out)?);
        }
        frames.retain(|f| !f.is_empty());
        Ok(frames)
    }
}
