//! Procedural grass in mouse-through, non-activating, capture-excluded layered windows.
use crate::platform::wide;
use std::{
    mem::zeroed,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*, Graphics::Gdi::*, System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};
const KEY: u32 = 0x00ff00ff;
struct Render {
    progress: f32,
}
struct Layer {
    hwnd: HWND,
    render: *mut Render,
}
pub struct Grass {
    layers: Vec<Layer>,
    progress: f32,
}
fn color(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
pub fn opacity(progress: f32) -> u8 {
    let p = progress.clamp(0.0, 1.0);
    (20.0 + 235.0 * p * p * (3.0 - 2.0 * p)) as u8
}
pub fn frontier(x: f32, y: f32, w: f32, h: f32) -> f32 {
    let dx = x.min(w - x);
    let dy = y.min(h - y);
    (dx * dx + dy * dy).sqrt() / ((w * w + h * h).sqrt() * 0.5)
}
fn paint(dc: HDC, w: i32, h: i32, p: f32, background: u32) {
    unsafe {
        let bg = CreateSolidBrush(background);
        let bounds = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(dc, &bounds, bg);
        DeleteObject(bg);
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let palette = [
            color(15, 44, 26),
            color(19, 65, 32),
            color(39, 106, 45),
            color(84, 142, 47),
            color(129, 165, 58),
        ];
        let mut old_brush = null_mut();
        for (pass, c) in palette.into_iter().enumerate() {
            let brush = CreateSolidBrush(c);
            let prev = SelectObject(dc, brush);
            if pass == 0 {
                old_brush = prev;
            }
            for y in (0..h + 24).step_by(24) {
                for x in (0..w + 24).step_by(24) {
                    let seed =
                        (x as u32).wrapping_mul(73856093) ^ (y as u32).wrapping_mul(19349663);
                    let jitter = (seed % 100) as f32 / 100.0 * 0.025;
                    let distance = frontier(x as f32, y as f32, w as f32, h as f32);
                    if p < 0.999 && distance + jitter > p {
                        continue;
                    }
                    if pass == 0 {
                        let r = RECT {
                            left: x - 14,
                            top: y - 14,
                            right: x + 15,
                            bottom: y + 15,
                        };
                        FillRect(dc, &r, brush);
                    } else {
                        for blade in 0..3 {
                            let mut s = seed.wrapping_add((pass * 137 + blade * 971) as u32);
                            s = (s ^ (s >> 16)).wrapping_mul(0x7feb352d);
                            s = (s ^ (s >> 15)).wrapping_mul(0x846ca68b);
                            s ^= s >> 16;
                            let bx = x - 12 + (s % 25) as i32;
                            let by = y - 10 + ((s >> 8) % 25) as i32;
                            let height = 10 + (s % 31) as i32;
                            let bend = (s % 19) as i32 - 9;
                            let breadth = 1 + (s % 3) as i32;
                            let points = [
                                POINT {
                                    x: bx - breadth,
                                    y: by,
                                },
                                POINT {
                                    x: bx + bend / 2 - 1,
                                    y: by - height / 2,
                                },
                                POINT {
                                    x: bx + bend,
                                    y: by - height,
                                },
                                POINT {
                                    x: bx + bend / 2 + 2,
                                    y: by - height / 2,
                                },
                                POINT {
                                    x: bx + breadth,
                                    y: by,
                                },
                            ];
                            Polygon(dc, points.as_ptr(), points.len() as i32);
                        }
                    }
                }
            }
            SelectObject(dc, prev);
            DeleteObject(brush);
        }
        if !old_brush.is_null() {
            SelectObject(dc, old_brush);
        }
        SelectObject(dc, old_pen);
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => HTTRANSPARENT as isize,
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut r: RECT = zeroed();
                GetClientRect(hwnd, &mut r);
                let render = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Render;
                let memory = CreateCompatibleDC(dc);
                let bitmap = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
                let previous = SelectObject(memory, bitmap);
                paint(
                    memory,
                    r.right,
                    r.bottom,
                    if render.is_null() {
                        0.0
                    } else {
                        (*render).progress
                    },
                    KEY,
                );
                BitBlt(dc, 0, 0, r.right, r.bottom, memory, 0, 0, SRCCOPY);
                SelectObject(memory, previous);
                DeleteObject(bitmap);
                DeleteDC(memory);
                EndPaint(hwnd, &ps);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}
unsafe extern "system" fn collect(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> i32 {
    unsafe {
        (&mut *(data as *mut Vec<RECT>)).push(*r);
    }
    1
}
impl Grass {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let name = wide("WindowsAwakeGrassLayer");
            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                hInstance: instance,
                lpszClassName: name.as_ptr(),
                ..zeroed()
            };
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err("无法创建草地窗口类".into());
            }
            let mut rects: Vec<RECT> = Vec::new();
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
            let mut grass = Self {
                layers: Vec::new(),
                progress: -1.0,
            };
            for r in rects {
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED
                        | WS_EX_TRANSPARENT
                        | WS_EX_NOACTIVATE
                        | WS_EX_TOOLWINDOW
                        | WS_EX_TOPMOST,
                    name.as_ptr(),
                    wide("Windows Awake Grass").as_ptr(),
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
                    return Err("无法创建草地覆盖层".into());
                }
                let render = Box::into_raw(Box::new(Render { progress: 0.0 }));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, render as isize);
                grass.layers.push(Layer { hwnd, render });
                if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) == 0 {
                    return Err("截图排除不可用，已跳过草地和熄屏，继续普通保活".into());
                }
                SetLayeredWindowAttributes(hwnd, KEY, 20, LWA_ALPHA | LWA_COLORKEY);
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            Ok(grass)
        }
    }
    pub fn update(&mut self, progress: f32) {
        let p = progress.clamp(0.0, 1.0);
        if (self.progress - p).abs() < 0.0005 {
            return;
        }
        self.progress = p;
        unsafe {
            for layer in &self.layers {
                (*layer.render).progress = p;
                SetLayeredWindowAttributes(layer.hwnd, KEY, opacity(p), LWA_ALPHA | LWA_COLORKEY);
                InvalidateRect(layer.hwnd, null(), 0);
                UpdateWindow(layer.hwnd);
            }
        }
    }
}
impl Drop for Grass {
    fn drop(&mut self) {
        unsafe {
            for layer in self.layers.drain(..) {
                DestroyWindow(layer.hwnd);
                drop(Box::from_raw(layer.render));
            }
        }
    }
}

/// Render the actual GDI painter offscreen for visual QA; never affects the desktop.
pub fn preview(path: &str) -> std::io::Result<()> {
    use std::io::Write;
    unsafe {
        let dc = CreateCompatibleDC(null_mut());
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = 1280;
        info.bmiHeader.biHeight = -720;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(dc);
            return Err(std::io::Error::other("DIB creation failed"));
        }
        let previous = SelectObject(dc, bitmap);
        let steps = [0.15, 0.45, 0.75, 1.0];
        for (i, p) in steps.into_iter().enumerate() {
            let saved = SaveDC(dc);
            SetViewportOrgEx(dc, (i % 2) as i32 * 640, (i / 2) as i32 * 360, null_mut());
            IntersectClipRect(dc, 0, 0, 640, 360);
            paint(dc, 640, 360, p, KEY);
            RestoreDC(dc, saved);
        }
        GdiFlush();
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, 1280 * 720 * 4);
        for y in 0..720 {
            for x in 0..1280 {
                let offset = (y * 1280 + x) * 4;
                let p = steps[(y / 360) * 2 + x / 640];
                let a = opacity(p) as u32;
                let transparent =
                    pixels[offset] == 255 && pixels[offset + 1] == 0 && pixels[offset + 2] == 255;
                for (j, bg) in [38u32, 30, 24].into_iter().enumerate() {
                    pixels[offset + j] = if transparent {
                        bg as u8
                    } else {
                        ((pixels[offset + j] as u32 * a + bg * (255 - a)) / 255) as u8
                    };
                }
                pixels[offset + 3] = 255;
            }
        }
        let mut bytes = Vec::with_capacity(54 + pixels.len());
        bytes.extend_from_slice(b"BM");
        bytes.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&54u32.to_le_bytes());
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(&1280i32.to_le_bytes());
        bytes.extend_from_slice(&(-720i32).to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&32u16.to_le_bytes());
        bytes.extend_from_slice(&[0; 24]);
        bytes.extend_from_slice(pixels);
        SelectObject(dc, previous);
        DeleteObject(bitmap);
        DeleteDC(dc);
        std::fs::File::create(path)?.write_all(&bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grows_from_four_corners_and_opacity_is_monotone() {
        for (x, y) in [(0., 0.), (100., 0.), (0., 80.), (100., 80.)] {
            assert_eq!(frontier(x, y, 100., 80.), 0.0)
        }
        assert!((frontier(50., 40., 100., 80.) - 1.0).abs() < 0.001);
        let mut last = 0;
        for i in 0..=100 {
            let a = opacity(i as f32 / 100.);
            assert!(a >= last);
            last = a;
        }
        assert_eq!(last, 255);
    }
}
