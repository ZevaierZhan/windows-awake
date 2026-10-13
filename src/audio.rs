//! Output level / session state only: no audio capture, recording or process-name heuristics.
use windows::Win32::{
    Media::Audio::{Endpoints::IAudioMeterInformation, *},
    System::Com::*,
};
pub struct AudioMonitor {
    enumerator: Option<IMMDeviceEnumerator>,
}
impl AudioMonitor {
    pub fn new() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| e.to_string())?;
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(enumerator) => Ok(Self {
                    enumerator: Some(enumerator),
                }),
                Err(err) => {
                    CoUninitialize();
                    Err(err.to_string())
                }
            }
        }
    }
    pub fn playing(&self) -> Result<bool, String> {
        self.query().map_err(|e| format!("音频状态检测不可用：{e}"))
    }
    fn query(&self) -> windows::core::Result<bool> {
        unsafe {
            let devices = self
                .enumerator
                .as_ref()
                .unwrap()
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            for i in 0..devices.GetCount()? {
                let device = devices.Item(i)?;
                let meter: IAudioMeterInformation = device.Activate(CLSCTX_ALL, None)?;
                if meter.GetPeakValue()? > 0.001 {
                    return Ok(true);
                }
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let sessions = manager.GetSessionEnumerator()?;
                for j in 0..sessions.GetCount()? {
                    if sessions.GetSession(j)?.GetState()? == AudioSessionStateActive {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
    }
}
impl Drop for AudioMonitor {
    fn drop(&mut self) {
        self.enumerator.take();
        unsafe {
            CoUninitialize();
        }
    }
}
