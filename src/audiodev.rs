//! Recording devices (microphones), shared by the recorder and the window.
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
    }
}

unsafe fn describe(dev: &IMMDevice) -> Option<AudioDevice> {
    let idp: PWSTR = dev.GetId().ok()?;
    let id = idp.to_string().unwrap_or_default();
    CoTaskMemFree(Some(idp.0 as _));
    let name = dev
        .OpenPropertyStore(STGM_READ)
        .and_then(|p| p.GetValue(&PKEY_Device_FriendlyName))
        .map(|v| v.to_string())
        .unwrap_or_default();
    Some(AudioDevice { name: if name.is_empty() { "Microphone".into() } else { name }, id })
}

/// Active microphones / line-ins, as Windows lists them.
pub fn microphones() -> Vec<AudioDevice> {
    let Ok(e) = enumerator() else { return Vec::new() };
    unsafe {
        let Ok(list) = e.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else { return Vec::new() };
        let n = list.GetCount().unwrap_or(0);
        (0..n).filter_map(|i| list.Item(i).ok()).filter_map(|d| describe(&d)).collect()
    }
}

/// Windows' default recording device.
pub fn default_microphone() -> Option<AudioDevice> {
    let e = enumerator().ok()?;
    unsafe { describe(&e.GetDefaultAudioEndpoint(eCapture, eConsole).ok()?) }
}

/// A specific device by id, or the default when `id` is None.
pub fn open_capture(id: Option<&str>) -> windows::core::Result<IMMDevice> {
    let e = enumerator()?;
    unsafe {
        match id {
            Some(id) => e.GetDevice(&HSTRING::from(id)),
            None => e.GetDefaultAudioEndpoint(eCapture, eConsole),
        }
    }
}

/// Human explanation for a failure to open a microphone.
pub fn explain(err: &windows::core::Error) -> String {
    if err.code() == windows::Win32::Foundation::E_ACCESSDENIED {
        "blocked by Windows privacy settings".into()
    } else if err.code().0 as u32 == 0x8889_0004 {
        // AUDCLNT_E_DEVICE_INVALIDATED
        "device unplugged".into()
    } else if err.code().0 as u32 == 0x8007_0490 {
        // ERROR_NOT_FOUND
        "device not found".into()
    } else {
        err.message().to_string()
    }
}
