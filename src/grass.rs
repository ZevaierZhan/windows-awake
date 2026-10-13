//! Continuous ink in capture-excluded, click-through DirectComposition windows.
use crate::{
    activity,
    ink_gpu::{Composition, Gpu},
    platform::{log, wide},
};
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};
struct Timeline {
    progress: f32,
    at: Instant,
    duration: f32,
}
struct Control {
    cancelled: AtomicBool,
    timeline: Mutex<Timeline>,
    error: Mutex<Option<String>>,
}
pub struct Grass {
    control: Arc<Control>,
    wake: mpsc::Sender<()>,
}
struct Layer {
    hwnd: HWND,
    composition: Composition,
}
impl Drop for Layer {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}
struct Window(HWND);
impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.0);
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as isize,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
unsafe extern "system" fn collect(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> i32 {
    unsafe {
        (&mut *(data as *mut Vec<RECT>)).push(*r);
    }
    1
}
fn create_window(r: RECT) -> Result<Window, String> {
    unsafe {
        let name = wide("WindowsAwakeInkLayer");
        let instance = GetModuleHandleW(null());
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return Err("无法创建水墨窗口类".into());
        }
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED
                | WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_TRANSPARENT
                | WS_EX_NOACTIVATE
                | WS_EX_TOOLWINDOW
                | WS_EX_TOPMOST,
            name.as_ptr(),
            wide("Windows Awake Ink").as_ptr(),
            WS_POPUP,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err("无法创建水墨覆盖层".into());
        }
        let window = Window(hwnd);
        // Layered + transparent is required for cross-process input passthrough;
        // DirectComposition owns the visual alpha, the window itself stays fully transparent.
        if SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) == 0 {
            return Err("无法启用鼠标穿透覆盖层".into());
        }
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
            return Err("截图排除不可用，跳过水墨和熄屏，继续普通保活".into());
        }
        Ok(window)
    }
}
impl Grass {
    pub fn new(duration_seconds: u32) -> Result<Self, String> {
        let control = Arc::new(Control {
            cancelled: AtomicBool::new(false),
            timeline: Mutex::new(Timeline {
                progress: 0.,
                at: Instant::now(),
                duration: duration_seconds as f32,
            }),
            error: Mutex::new(None),
        });
        let (wake, receiver) = mpsc::channel();
        let thread_control = control.clone();
        let sequence = activity::snapshot().sequence;
        std::thread::Builder::new()
            .name("ink-render".into())
            .spawn(move || {
                if let Err(e) = render(&thread_control, receiver, sequence) {
                    log(&e);
                    *thread_control.error.lock().unwrap() = Some(e);
                }
            })
            .map_err(|e| format!("无法启动水墨渲染线程：{e}"))?;
        Ok(Self { control, wake })
    }
    pub fn update(&mut self, progress: f32) -> Result<(), String> {
        if let Some(e) = self.control.error.lock().unwrap().clone() {
            return Err(e);
        }
        let mut time = self.control.timeline.lock().unwrap();
        time.progress = progress.clamp(0., 1.);
        time.at = Instant::now();
        Ok(())
    }
}
impl Drop for Grass {
    fn drop(&mut self) {
        self.control.cancelled.store(true, Ordering::Release);
        let _ = self.wake.send(());
    }
}
fn render(control: &Control, wake: mpsc::Receiver<()>, sequence: u64) -> Result<(), String> {
    unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_MULTITHREADED,
        )
        .ok()
        .map_err(|e| e.to_string())?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                windows::Win32::System::Com::CoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let cancelled =
        || control.cancelled.load(Ordering::Acquire) || activity::snapshot().sequence != sequence;
    if cancelled() {
        return Ok(());
    }
    let mut gpu = Gpu::new()?;
    let mut rects: Vec<RECT> = Vec::new();
    unsafe {
        if EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            &mut rects as *mut _ as isize,
        ) == 0
            || rects.is_empty()
        {
            return Err("无法读取屏幕布局".into());
        }
    }
    let mut layers = Vec::new();
    for rect in rects {
        if cancelled() {
            return Ok(());
        }
        let window = create_window(rect)?;
        let composition = Composition::new(
            &gpu,
            window.0,
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        )
        .map_err(|e| format!("无法创建 GPU 合成层：{e}"))?;
        let hwnd = window.0;
        std::mem::forget(window);
        layers.push(Layer { hwnd, composition });
    }
    let mut shown = false;
    let mut previous = -1.;
    loop {
        let frame_started = Instant::now();
        if cancelled() {
            return Ok(());
        }
        unsafe {
            let mut msg: MSG = zeroed();
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let p = {
            let time = control.timeline.lock().unwrap();
            (time.progress + time.at.elapsed().as_secs_f32() / time.duration).min(1.)
        };
        if p != previous {
            gpu.prepare(p)?;
            if cancelled() {
                return Ok(());
            }
            for layer in &layers {
                layer
                    .composition
                    .present(&gpu, p)
                    .map_err(|e| format!("GPU 水墨呈现失败：{e}"))?;
            }
            if !shown {
                if cancelled() {
                    return Ok(());
                }
                for layer in &layers {
                    unsafe {
                        ShowWindow(layer.hwnd, SW_SHOWNOACTIVATE);
                    }
                }
                shown = true;
                log("ink renderer ready: hardware D3D11, 24 fps, continuous source frames");
            }
            previous = p;
        }
        // Decode/render time counts towards the 24 fps budget.
        let remaining = Duration::from_micros(41_667).saturating_sub(frame_started.elapsed());
        let _ = wake.recv_timeout(remaining);
    }
}
fn write_bmp(path: &std::path::Path, w: u32, h: u32, mut pixels: Vec<u8>) -> Result<(), String> {
    for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let y = (i / w as usize) as u32;
        let x = (i % w as usize) as u32;
        let alpha = pixel[3] as u32;
        let gradient = (x * 18 / w + y * 12 / h) as u8;
        for (channel, background) in [92u8, 65, 44].into_iter().enumerate() {
            pixel[channel] = (pixel[channel] as u32
                + (background + gradient) as u32 * (255 - alpha) / 255)
                .min(255) as u8;
        }
        pixel[3] = 255;
    }
    let mut bytes = Vec::with_capacity(54 + pixels.len());
    bytes.extend_from_slice(b"BM");
    bytes.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 4]);
    bytes.extend_from_slice(&54u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&(w as i32).to_le_bytes());
    bytes.extend_from_slice(&(-(h as i32)).to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&32u16.to_le_bytes());
    bytes.extend_from_slice(&[0; 24]);
    bytes.extend_from_slice(&pixels);
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}
/// Offline native GPU preview. No desktop overlay, audio, input or power operations.
pub fn preview(path: &str) -> Result<(), String> {
    let mut gpu = Gpu::new()?;
    let surface = gpu.surface(640, 360).map_err(|e| e.to_string())?;
    let mut pixels = vec![0; 1280 * 720 * 4];
    for (index, p) in [0.15, 0.45, 0.75, 1.].into_iter().enumerate() {
        gpu.prepare(p)?;
        gpu.draw(&surface.target, 640, 360, p);
        let frame = gpu.read(&surface).map_err(|e| e.to_string())?;
        for y in 0..360 {
            let dst = ((index / 2 * 360 + y) * 1280 + index % 2 * 640) * 4;
            pixels[dst..dst + 640 * 4].copy_from_slice(&frame[y * 640 * 4..(y + 1) * 640 * 4]);
        }
    }
    write_bmp(std::path::Path::new(path), 1280, 720, pixels)
}
pub fn preview_frames(path: &str) -> Result<(), String> {
    let root = std::path::Path::new(path);
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut gpu = Gpu::new()?;
    let surface = gpu.surface(960, 540).map_err(|e| e.to_string())?;
    for index in 0..=144 {
        let p = index as f32 / 144.;
        gpu.prepare(p)?;
        gpu.draw(&surface.target, 960, 540, p);
        write_bmp(
            &root.join(format!("frame-{index:04}.bmp")),
            960,
            540,
            gpu.read(&surface).map_err(|e| e.to_string())?,
        )?;
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Requires real hardware D3D11; offscreen, no input/power/monitor actions"]
    fn native_shader_starts_transparent_and_finishes_opaque() {
        let mut gpu = Gpu::new().unwrap();
        let surface = gpu.surface(128, 72).unwrap();
        let mut image = |p| {
            gpu.prepare(p).unwrap();
            gpu.draw(&surface.target, 128, 72, p);
            gpu.read(&surface).unwrap()
        };
        assert!(
            image(0.)
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| *pixel == [0; 4])
        );
        assert!(
            image(1.)
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
        let a = image(0.45);
        let b = image(0.451);
        let change: f32 = a
            .iter()
            .zip(b)
            .map(|(a, b)| a.abs_diff(b) as f32)
            .sum::<f32>()
            / a.len() as f32;
        assert!(
            change < 3.,
            "Adjacent animation moments jumped by {change} levels"
        );
    }
    #[test]
    #[ignore = "Requires real D3D11 hardware and a Windows desktop; no input, power or monitor actions"]
    fn native_composition_window_preserves_focus_and_click_through() {
        unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
            .ok()
            .unwrap();
        }
        {
            let mut gpu = Gpu::new().unwrap();
            let hit = POINT { x: 1, y: 1 };
            let underneath = unsafe { WindowFromPoint(hit) };
            let window = create_window(RECT {
                left: 0,
                top: 0,
                right: 2,
                bottom: 2,
            })
            .unwrap();
            let focus = unsafe { GetForegroundWindow() };
            let composition = Composition::new(&gpu, window.0, 2, 2).unwrap();
            gpu.prepare(0.).unwrap();
            composition.present(&gpu, 0.).unwrap();
            unsafe {
                ShowWindow(window.0, SW_SHOWNOACTIVATE);
                assert_eq!(GetForegroundWindow(), focus);
                assert_eq!(WindowFromPoint(hit), underneath);
                assert_ne!(WindowFromPoint(hit), window.0);
                assert_eq!(
                    GetWindowLongPtrW(window.0, GWL_EXSTYLE) as u32
                        & (WS_EX_LAYERED | WS_EX_TRANSPARENT),
                    WS_EX_LAYERED | WS_EX_TRANSPARENT
                );
                assert_eq!(
                    SendMessageW(window.0, WM_NCHITTEST, 0, 0),
                    HTTRANSPARENT as isize
                );
                assert_eq!(
                    SendMessageW(window.0, WM_MOUSEACTIVATE, 0, 0),
                    MA_NOACTIVATE as isize
                );
                let mut affinity = 0;
                assert_ne!(GetWindowDisplayAffinity(window.0, &mut affinity), 0);
                assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE);
            }
        }
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
    }
}
