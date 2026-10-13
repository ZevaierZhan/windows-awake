//! Only event timing and injection metadata are observed; never key values or cursor positions.
use std::{
    mem::zeroed,
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount64,
        Threading::GetCurrentThreadId,
    },
    UI::WindowsAndMessaging::*,
};
pub const INPUT_MESSAGE: u32 = WM_APP + 2;
static OWNER: AtomicUsize = AtomicUsize::new(0);
static PENDING: AtomicBool = AtomicBool::new(false);
static LAST_MS: AtomicU64 = AtomicU64::new(0);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PHYSICAL: AtomicU64 = AtomicU64::new(0);
static INJECTED: AtomicU64 = AtomicU64::new(0);
static OWN_TICK: AtomicU32 = AtomicU32::new(0);
static HEARTBEAT: AtomicU64 = AtomicU64::new(0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Own,
    Physical,
    Automation,
}
pub fn classify(extra: usize, injected: bool) -> Source {
    if extra == crate::platform::keepalive_tag() {
        Source::Own
    } else if injected {
        Source::Automation
    } else {
        Source::Physical
    }
}
fn record(source: Source, tick: u32) {
    if source == Source::Own {
        OWN_TICK.store(tick, Ordering::SeqCst);
        return;
    }
    let seq = SEQUENCE.load(Ordering::SeqCst) + 1;
    LAST_MS.store(unsafe { GetTickCount64() }, Ordering::SeqCst);
    match source {
        Source::Physical => PHYSICAL.store(seq, Ordering::SeqCst),
        Source::Automation => INJECTED.store(seq, Ordering::SeqCst),
        Source::Own => {}
    }
    SEQUENCE.store(seq, Ordering::SeqCst);
    if !PENDING.swap(true, Ordering::SeqCst) {
        unsafe {
            PostMessageW(OWNER.load(Ordering::SeqCst) as HWND, INPUT_MESSAGE, 0, 0);
        }
    }
}
unsafe extern "system" fn mouse(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && l != 0 {
        let event = unsafe { &*(l as *const MSLLHOOKSTRUCT) };
        record(
            classify(event.dwExtraInfo, event.flags & LLMHF_INJECTED != 0),
            event.time,
        );
    }
    unsafe { CallNextHookEx(null_mut(), code, w, l) }
}
unsafe extern "system" fn keyboard(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && l != 0 {
        let event = unsafe { &*(l as *const KBDLLHOOKSTRUCT) };
        record(
            classify(event.dwExtraInfo, event.flags & LLKHF_INJECTED != 0),
            event.time,
        );
    }
    unsafe { CallNextHookEx(null_mut(), code, w, l) }
}
pub struct Snapshot {
    pub sequence: u64,
    pub last_ms: u64,
    pub automation: bool,
    pub own_tick: u32,
    pub healthy: bool,
}
pub fn snapshot() -> Snapshot {
    let now = unsafe { GetTickCount64() };
    Snapshot {
        sequence: SEQUENCE.load(Ordering::SeqCst),
        last_ms: LAST_MS.load(Ordering::SeqCst),
        automation: INJECTED.load(Ordering::SeqCst) > PHYSICAL.load(Ordering::SeqCst),
        own_tick: OWN_TICK.load(Ordering::SeqCst),
        healthy: now.saturating_sub(HEARTBEAT.load(Ordering::SeqCst)) < 3500,
    }
}
pub fn acknowledge() {
    PENDING.store(false, Ordering::SeqCst)
}
pub fn expect_own_tick(tick: u32) {
    OWN_TICK.store(tick, Ordering::SeqCst);
}
pub struct Watcher {
    id: u32,
    join: Option<JoinHandle<()>>,
}
impl Watcher {
    pub fn start(hwnd: HWND) -> Result<Self, String> {
        OWNER.store(hwnd as usize, Ordering::SeqCst);
        LAST_MS.store(unsafe { GetTickCount64() }, Ordering::SeqCst);
        let (tx, rx) = mpsc::sync_channel::<Result<u32, String>>(1);
        let join = thread::spawn(move || unsafe {
            let mut msg: MSG = zeroed();
            PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
            let m = SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse),
                GetModuleHandleW(std::ptr::null()),
                0,
            );
            let k = SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard),
                GetModuleHandleW(std::ptr::null()),
                0,
            );
            if m.is_null() || k.is_null() {
                if !m.is_null() {
                    UnhookWindowsHookEx(m);
                }
                if !k.is_null() {
                    UnhookWindowsHookEx(k);
                }
                let _ = tx.send(Err("无法安装输入检测，已停止保活".into()));
                return;
            }
            let timer = SetTimer(null_mut(), 1, 1000, None);
            HEARTBEAT.store(GetTickCount64(), Ordering::SeqCst);
            if timer == 0 || tx.send(Ok(GetCurrentThreadId())).is_err() {
                UnhookWindowsHookEx(m);
                UnhookWindowsHookEx(k);
                return;
            }
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                if msg.message == WM_TIMER {
                    HEARTBEAT.store(GetTickCount64(), Ordering::SeqCst);
                }
            }
            KillTimer(null_mut(), timer);
            UnhookWindowsHookEx(m);
            UnhookWindowsHookEx(k);
            HEARTBEAT.store(0, Ordering::SeqCst);
        });
        let id = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "输入检测启动超时")??;
        Ok(Self {
            id,
            join: Some(join),
        })
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.id, WM_QUIT, 0, 0);
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        OWNER.store(0, Ordering::SeqCst);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn own_pulses_are_not_user_or_automation() {
        assert_eq!(
            classify(crate::platform::keepalive_tag(), true),
            Source::Own
        );
        assert_eq!(classify(0, true), Source::Automation);
        assert_eq!(classify(0, false), Source::Physical);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    unsafe extern "system" fn default_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(h, m, w, l) }
    }
    #[test]
    #[ignore = "Live metadata probe: zero displacement and one F24 press/release; no overlay or screen power changes"]
    fn live_hook_and_own_timing() {
        unsafe {
            let name = crate::platform::wide("WindowsAwakeInputProbe");
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(default_proc),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..zeroed()
            };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            let watcher = Watcher::start(hwnd).unwrap();
            let before = snapshot();
            crate::platform::pulse().unwrap();
            thread::sleep(Duration::from_millis(150));
            let own = snapshot();
            let stamp = crate::platform::last_input_stamp().unwrap();
            println!(
                "own pulse sequence={} -> {}, own_tick={}, stamp={}, healthy={}",
                before.sequence, own.sequence, own.own_tick, stamp, own.healthy
            );
            assert!(own.healthy);
            if own.sequence == before.sequence {
                assert_eq!(own.own_tick, stamp);
            }
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dwFlags: MOUSEEVENTF_MOVE,
                        ..zeroed()
                    },
                },
            };
            assert_eq!(SendInput(1, &input, std::mem::size_of::<INPUT>() as i32), 1);
            thread::sleep(Duration::from_millis(150));
            let other = snapshot();
            println!(
                "external zero sequence={} automation={}",
                other.sequence, other.automation
            );
            let key = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_F24,
                        ..zeroed()
                    },
                },
            };
            let up = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VK_F24,
                        dwFlags: KEYEVENTF_KEYUP,
                        ..zeroed()
                    },
                },
            };
            let injected_before = INJECTED.load(Ordering::SeqCst);
            assert_eq!(
                SendInput(2, [key, up].as_ptr(), std::mem::size_of::<INPUT>() as i32),
                2
            );
            thread::sleep(Duration::from_millis(50));
            let keyboard = snapshot();
            println!(
                "external keyboard sequence={} automation={}",
                keyboard.sequence, keyboard.automation
            );
            assert!(keyboard.sequence > other.sequence);
            assert!(INJECTED.load(Ordering::SeqCst) > injected_before);
            drop(watcher);
            DestroyWindow(hwnd);
        }
    }
}
