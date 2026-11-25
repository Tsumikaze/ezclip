//! Monitor enumeration via DXGI (same order the recorder uses).
use windows::Win32::Graphics::Dxgi::*;

pub struct Monitor {
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput1,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

pub fn monitors() -> Vec<Monitor> {
    let mut list = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return list };
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let Ok(desc) = output.GetDesc() else { continue };
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let Ok(output1) = windows::core::Interface::cast::<IDXGIOutput1>(&output) else { continue };
                let r = desc.DesktopCoordinates;
                list.push(Monitor {
                    adapter: adapter.clone(),
                    output: output1,
                    width: (r.right - r.left) as u32,
                    height: (r.bottom - r.top) as u32,
                    primary: r.left == 0 && r.top == 0,
                });
            }
        }
    }
    // Primary monitor first so the default (index 0) is the one people expect.
    list.sort_by_key(|m| !m.primary);
    list
}

pub fn label(i: usize, m: &Monitor) -> String {
    format!(
        "Monitor {} — {}×{}{}",
        i + 1,
        m.width,
        m.height,
        if m.primary { " (primary)" } else { "" }
    )
}
