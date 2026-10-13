use std::mem::{size_of, zeroed};
use windows_sys::Win32::{
    Foundation::POINT, System::SystemInformation::GetTickCount, UI::Input::KeyboardAndMouse::*,
};
#[test]
#[ignore = "Live Windows integration: injects one zero-displacement input; keep pointer still"]
fn zero_displacement_refreshes_idle_without_moving_cursor() {
    unsafe {
        let mut before: LASTINPUTINFO = zeroed();
        before.cbSize = size_of::<LASTINPUTINFO>() as u32;
        assert_ne!(GetLastInputInfo(&mut before), 0);
        let mut p1: POINT = zeroed();
        assert_ne!(
            windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p1),
            0
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            crate::platform::interactive_desktop(),
            "test requires an unlocked connected Windows session"
        );
        crate::platform::pulse().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut after = before;
        assert_ne!(GetLastInputInfo(&mut after), 0);
        let mut p2: POINT = zeroed();
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p2);
        println!(
            "before={} after={} now={} cursor=({},{})->({},{})",
            before.dwTime,
            after.dwTime,
            GetTickCount(),
            p1.x,
            p1.y,
            p2.x,
            p2.y
        );
        assert_ne!(before.dwTime, after.dwTime);
        assert_eq!((p1.x, p1.y), (p2.x, p2.y));
    }
}
