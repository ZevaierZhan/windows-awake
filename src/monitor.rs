//! DDC/CI hardware power control. Never fall back to Windows software screen-off.
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{Devices::Display::*, Foundation::*, Graphics::Gdi::*};

pub struct Monitors {
    devices: Vec<Device>,
}
struct Device {
    raw: PHYSICAL_MONITOR,
    original: u32,
    touched: bool,
}
impl Drop for Device {
    fn drop(&mut self) {
        if self.touched
            && unsafe { SetVCPFeature(self.raw.hPhysicalMonitor, 0xd6, self.original) } == 0
        {
            crate::platform::log("DDC restore failed; use monitor's power button");
        }
        unsafe {
            DestroyPhysicalMonitor(self.raw.hPhysicalMonitor);
        }
    }
}
unsafe extern "system" fn enumerate(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
    // SAFETY: EnumDisplayMonitors invokes synchronously with the caller's Vec pointer.
    unsafe {
        let devices = &mut *(data as *mut Vec<Device>);
        let mut count = 0;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) == 0
            || count == 0
            || count > 16
        {
            return 0;
        }
        let mut raw: Vec<PHYSICAL_MONITOR> = (0..count).map(|_| zeroed()).collect();
        if GetPhysicalMonitorsFromHMONITOR(monitor, count, raw.as_mut_ptr()) == 0 {
            return 0;
        }
        for raw in raw {
            devices.push(Device {
                raw,
                original: 1,
                touched: false,
            });
        }
    }
    1
}

/// Require a capabilities string explicitly listing D6 value 04 (hardware off).
fn supports_off(capabilities: &str) -> bool {
    let compact: String = capabilities
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    let Some(start) = compact.find("d6(") else {
        return false;
    };
    let Some(end) = compact[start + 3..].find(')') else {
        return false;
    };
    let values = &compact[start + 3..start + 3 + end];
    values.len().is_multiple_of(2)
        && values.as_bytes().iter().all(u8::is_ascii_hexdigit)
        && values
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .any(|value| value == b"04")
}
impl Monitors {
    pub fn discover() -> Result<Self, String> {
        let mut devices: Vec<Device> = Vec::new();
        if unsafe {
            EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(enumerate),
                &mut devices as *mut _ as isize,
            )
        } == 0
            || devices.is_empty()
        {
            return Err("无法访问全部显示器的硬件电源接口。".into());
        }
        for device in &mut devices {
            let handle = device.raw.hPhysicalMonitor;
            let mut length = 0;
            if unsafe { GetCapabilitiesStringLength(handle, &mut length) } == 0
                || !(1..=16384).contains(&length)
            {
                return Err("显示器未提供 DDC/CI 能力信息，不能确认支持硬件熄屏。".into());
            }
            let mut capabilities = vec![0u8; length as usize];
            if unsafe {
                CapabilitiesRequestAndCapabilitiesReply(handle, capabilities.as_mut_ptr(), length)
            } == 0
                || !supports_off(&String::from_utf8_lossy(&capabilities))
            {
                return Err("显示器没有声明支持 DDC/CI 硬件熄屏（D6=04）。".into());
            }
            if unsafe {
                GetVCPFeatureAndVCPFeatureReply(
                    handle,
                    0xd6,
                    null_mut(),
                    &mut device.original,
                    null_mut(),
                )
            } == 0
                || device.original != 1
            {
                return Err("无法确认显示器处于开启状态，请先点亮显示器。".into());
            }
        }
        Ok(Self { devices })
    }
    pub fn off(&mut self) -> Result<(), String> {
        for device in &mut self.devices {
            // Even a reported failure may have reached hardware; arm restoration first.
            device.touched = true;
            if unsafe { SetVCPFeature(device.raw.hPhysicalMonitor, 0xd6, 4) } == 0 {
                return Err("显示器拒绝硬件熄屏；已尝试恢复原电源状态。".into());
            }
        }
        Ok(())
    }
    pub fn restore(&mut self) -> Result<(), String> {
        let mut failed = false;
        for device in &mut self.devices {
            if device.touched {
                if unsafe { SetVCPFeature(device.raw.hPhysicalMonitor, 0xd6, device.original) } != 0
                {
                    device.touched = false;
                } else {
                    failed = true;
                }
            }
        }
        if failed {
            Err("显示器未响应点亮命令，请使用显示器实体电源键。".into())
        } else {
            Ok(())
        }
    }
    #[cfg(test)]
    pub fn power_values(&self) -> Vec<Option<u32>> {
        self.devices
            .iter()
            .map(|device| {
                let mut value = 0;
                (unsafe {
                    GetVCPFeatureAndVCPFeatureReply(
                        device.raw.hPhysicalMonitor,
                        0xd6,
                        null_mut(),
                        &mut value,
                        null_mut(),
                    )
                } != 0)
                    .then_some(value)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_explicit_hardware_off_capability() {
        assert!(supports_off("(vcp(10 12 D6 (01 04 05)))"));
        assert!(!supports_off("(vcp(10 12 d6(01 02)))"));
        assert!(!supports_off("(vcp(10 12))"));
        assert!(!supports_off("(vcp(d6(0104"));
        assert!(!supports_off("(vcp(d6(104)))"));
    }
}
