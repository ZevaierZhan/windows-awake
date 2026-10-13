//! Explicit hardware probe; never run as part of ordinary tests.
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};
use windows_sys::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Power::*},
        UI::WindowsAndMessaging::*,
    },
    core::GUID,
};

static DISPLAY_STATE: AtomicU32 = AtomicU32::new(99);
static OFF_EVENTS: AtomicU32 = AtomicU32::new(0);
const DISPLAY_GUID: GUID = GUID::from_u128(0x2b84c20e_ad23_4ddf_93db_05ffbd7efca5);

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_POWERBROADCAST && w as u32 == PBT_POWERSETTINGCHANGE && l != 0 {
        let setting = l as *const POWERBROADCAST_SETTING;
        // The broadcast payload is valid for this callback; its Data array is variable length.
        unsafe {
            if (*setting).DataLength >= 4 {
                let guid = (*setting).PowerSetting;
                if guid.data1 == DISPLAY_GUID.data1
                    && guid.data2 == DISPLAY_GUID.data2
                    && guid.data3 == DISPLAY_GUID.data3
                    && guid.data4 == DISPLAY_GUID.data4
                {
                    let value =
                        std::ptr::read_unaligned(std::ptr::addr_of!((*setting).Data).cast::<u32>());
                    DISPLAY_STATE.store(value, Ordering::SeqCst);
                    if value == 0 {
                        OFF_EVENTS.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
        }
        return 1;
    }
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}
fn pump(ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < ms as u128 {
        unsafe {
            let mut msg: MSG = zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Restore(HWND, HPOWERNOTIFY, u32);
impl Drop for Restore {
    fn drop(&mut self) {
        unsafe {
            // Restore first, including assertion failures, then tear down the probe window.
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SYSCOMMAND,
                SC_MONITORPOWER as usize,
                -1,
                SMTO_ABORTIFHUNG,
                200,
                null_mut(),
            );
            UnregisterPowerSettingNotification(self.1);
            DestroyWindow(self.0);
            SetThreadExecutionState(self.2);
        }
    }
}

#[test]
#[ignore = "Hardware experiment: switches monitor off for several seconds then automatically restores it"]
fn screen_off_with_zero_input() {
    assert!(crate::platform::interactive_desktop());
    unsafe {
        let instance = GetModuleHandleW(null());
        let name = crate::platform::wide("WindowsAwakeDisplayProbe");
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..zeroed()
        };
        assert_ne!(RegisterClassW(&class), 0);
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
            null(),
        );
        assert!(!hwnd.is_null());
        let notify = RegisterPowerSettingNotification(hwnd, &DISPLAY_GUID, 0);
        assert_ne!(notify, 0);
        let old_power = SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
        let restore = Restore(hwnd, notify, old_power);
        pump(300);
        println!(
            "initial display state={}",
            DISPLAY_STATE.load(Ordering::SeqCst)
        );
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SYSCOMMAND,
            SC_MONITORPOWER as usize,
            2,
            SMTO_ABORTIFHUNG,
            200,
            null_mut(),
        );
        pump(1200);
        let before = DISPLAY_STATE.load(Ordering::SeqCst);
        let idle_before = crate::platform::last_input_idle().unwrap();
        crate::platform::pulse().unwrap();
        pump(2000);
        let after = DISPLAY_STATE.load(Ordering::SeqCst);
        let idle_after = crate::platform::last_input_idle().unwrap();
        println!(
            "display before pulse={before} after pulse={after}; idle_before={idle_before} idle_after={idle_after}; off_events={}",
            OFF_EVENTS.load(Ordering::SeqCst)
        );
        drop(restore);
        // Observation only: record unsupported hardware rather than asserting a universal result.
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PhysicalMonitor {
    handle: HANDLE,
    description: [u16; 128],
}
#[link(name = "dxva2")]
unsafe extern "system" {
    fn GetNumberOfPhysicalMonitorsFromHMONITOR(
        monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
        count: *mut u32,
    ) -> i32;
    fn GetPhysicalMonitorsFromHMONITOR(
        monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
        count: u32,
        monitors: *mut PhysicalMonitor,
    ) -> i32;
    fn GetVCPFeatureAndVCPFeatureReply(
        monitor: HANDLE,
        code: u8,
        kind: *mut i32,
        current: *mut u32,
        maximum: *mut u32,
    ) -> i32;
    fn DestroyPhysicalMonitors(count: u32, monitors: *const PhysicalMonitor) -> i32;
}
unsafe extern "system" fn inspect_monitor(
    monitor: windows_sys::Win32::Graphics::Gdi::HMONITOR,
    _: windows_sys::Win32::Graphics::Gdi::HDC,
    _: *mut RECT,
    _: LPARAM,
) -> i32 {
    unsafe {
        let mut count = 0;
        if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count) == 0 {
            println!(
                "physical monitor enumeration unavailable: {}",
                GetLastError()
            );
            return 1;
        }
        println!("physical monitor count={count}");
        if count == 0 || count > 16 {
            return 1;
        }
        let mut monitors = vec![zeroed::<PhysicalMonitor>(); count as usize];
        if GetPhysicalMonitorsFromHMONITOR(monitor, count, monitors.as_mut_ptr()) == 0 {
            println!("physical monitor handles unavailable: {}", GetLastError());
            return 1;
        }
        for device in &monitors {
            let mut current = 0;
            let ok = GetVCPFeatureAndVCPFeatureReply(
                device.handle,
                0xd6,
                null_mut(),
                &mut current,
                null_mut(),
            );
            let length = device
                .description
                .iter()
                .position(|&ch| ch == 0)
                .unwrap_or(128);
            println!(
                "monitor={} power_vcp_supported={} current={} error={}",
                String::from_utf16_lossy(&device.description[..length]),
                ok != 0,
                current,
                if ok == 0 { GetLastError() } else { 0 }
            );
        }
        DestroyPhysicalMonitors(count, monitors.as_ptr());
    }
    1
}
#[test]
#[ignore = "Read-only hardware diagnostic: queries DDC/CI monitor power support"]
fn inspect_ddc_power_support() {
    unsafe {
        assert_ne!(
            windows_sys::Win32::Graphics::Gdi::EnumDisplayMonitors(
                null_mut(),
                null(),
                Some(inspect_monitor),
                0
            ),
            0
        );
    }
}

#[test]
#[ignore = "Hardware experiment: DDC/CI off, zero-input keepalive, then automatic restoration"]
fn ddc_off_with_zero_input() {
    assert!(crate::platform::interactive_desktop());
    let mut monitors = match crate::monitor::Monitors::discover() {
        Ok(monitors) => monitors,
        Err(err) => {
            println!("DDC hardware-off unavailable: {err}");
            return;
        }
    };
    let mut power = crate::platform::PowerRequest::default();
    power.set(true).unwrap();
    monitors.off().unwrap();
    std::thread::sleep(Duration::from_millis(800));
    let before = monitors.power_values();
    crate::platform::pulse().unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let after = monitors.power_values();
    let idle = crate::platform::last_input_idle().unwrap();
    monitors.restore().unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    println!(
        "DDC before={before:?} after={after:?} idle_after={idle} restored={:?}",
        monitors.power_values()
    );
    assert!(before.iter().all(|value| *value == Some(4)));
    assert!(after.iter().all(|value| *value == Some(4)));
    assert!(idle < 2500);
}
