use std::{
    fs::{self, OpenOptions},
    io::Write,
    mem::{size_of, zeroed},
    ptr::null_mut,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        Power::*, RemoteDesktop::*, StationsAndDesktops::*, SystemInformation::GetTickCount,
        Threading::CreateMutexW,
    },
    UI::Input::KeyboardAndMouse::*,
};

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub fn external_power() -> crate::config::ExternalPower {
    // Only an explicit online ACLineStatus permits keep-awake, even on batteryless PCs.
    // SAFETY: the API synchronously fills a correctly sized SYSTEM_POWER_STATUS.
    unsafe {
        let mut status: SYSTEM_POWER_STATUS = zeroed();
        if GetSystemPowerStatus(&mut status) == 0 {
            return crate::config::ExternalPower::Unknown;
        }
        crate::config::ExternalPower::from_ac_status(status.ACLineStatus)
    }
}

pub fn log(text: &str) {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let dir = std::path::PathBuf::from(base).join("WindowsAwake");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("activity.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > 1_048_576) {
        let previous = dir.join("activity.previous.log");
        let _ = fs::remove_file(&previous);
        let _ = fs::rename(&path, previous);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(file, "{time} {text}");
    }
}

pub fn last_input_idle() -> Result<u32, String> {
    // SAFETY: correctly sized LASTINPUTINFO lives until the synchronous call returns.
    unsafe {
        let mut info: LASTINPUTINFO = zeroed();
        info.cbSize = size_of::<LASTINPUTINFO>() as u32;
        if GetLastInputInfo(&mut info) == 0 {
            return Err(format!("GetLastInputInfo: {}", GetLastError()));
        }
        Ok(crate::config::idle_millis(GetTickCount(), info.dwTime))
    }
}

pub fn pulse() -> Result<(), String> {
    // Relative zero displacement: never read/restore coordinates, click, scroll or press keys.
    // A remote controller may move the pointer concurrently without a stale-coordinate race.
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0x41574B45,
            },
        },
    };
    // SAFETY: input is initialized and cbSize matches the architecture's INPUT ABI.
    if unsafe { SendInput(1, &input, size_of::<INPUT>() as i32) } != 1 {
        return Err(format!(
            "SendInput rejected (possible UIPI/desktop restriction): {}",
            unsafe { GetLastError() }
        ));
    }
    Ok(())
}

pub fn interactive_desktop() -> bool {
    // Fail closed if the session or input desktop cannot be established.
    unsafe {
        let mut buffer = null_mut();
        let mut bytes = 0;
        if WTSQuerySessionInformationW(
            null_mut(),
            WTS_CURRENT_SESSION,
            WTSConnectState,
            &mut buffer,
            &mut bytes,
        ) == 0
        {
            return false;
        }
        let active = !buffer.is_null()
            && bytes >= size_of::<WTS_CONNECTSTATE_CLASS>() as u32
            && *(buffer as *const WTS_CONNECTSTATE_CLASS) == WTSActive;
        WTSFreeMemory(buffer.cast());
        if !active {
            return false;
        }
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return false;
        }
        let mut name = [0u16; 256];
        let mut needed = 0;
        let ok = GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            size_of_val(&name) as u32,
            &mut needed,
        );
        CloseDesktop(desktop);
        if ok == 0 {
            return false;
        }
        let length = name.iter().position(|&v| v == 0).unwrap_or(name.len());
        String::from_utf16_lossy(&name[..length]).eq_ignore_ascii_case("Default")
    }
}

/// All methods must run on the owning message-loop thread: Windows tracks this per thread.
#[derive(Default)]
pub struct PowerRequest {
    active: bool,
}
impl PowerRequest {
    pub fn set(&mut self, active: bool) -> Result<(), String> {
        if active == self.active {
            return Ok(());
        }
        let flags = ES_CONTINUOUS
            | if active {
                ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
            } else {
                0
            };
        if unsafe { SetThreadExecutionState(flags) } == 0 {
            return Err(format!("SetThreadExecutionState: {}", unsafe {
                GetLastError()
            }));
        }
        self.active = active;
        log(&format!("power_request={active}"));
        Ok(())
    }
}
impl Drop for PowerRequest {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

pub struct Instance(HANDLE);
impl Instance {
    pub fn acquire() -> Result<Self, String> {
        // Local namespace intentionally permits one instance per interactive session.
        let handle =
            unsafe { CreateMutexW(null_mut(), 0, wide("Local\\WindowsAwake.0.1").as_ptr()) };
        if handle.is_null() {
            return Err("创建单实例锁失败".into());
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            return Err("Windows Awake 已在本会话运行，请查看系统托盘。".into());
        }
        Ok(Self(handle))
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
