#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod activity;
mod audio;
mod config;
#[cfg(test)]
mod display_probe;
mod grass;
mod ink_asset;
mod ink_gpu;
mod monitor;
mod platform;
#[cfg(test)]
mod probe;
use config::{Config, ExternalPower, Phase};
use platform::{Instance, PowerRequest, log, wide};
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW, RemoteDesktop::*, SystemInformation::GetTickCount64,
    },
    UI::{HiDpi::*, Shell::*, WindowsAndMessaging::*},
};
const TRAY_MESSAGE: u32 = WM_APP + 1;
const TOGGLE: u32 = 100;
const POWER_ONLY: u32 = 101;
const EXIT: u32 = 102;
const STATUS: u32 = 103;
const HELP: &str = "Windows Awake 0.3\n\n仅接电且无人操作时进入保活。默认：空闲30秒→仅保活30秒→四角水墨扩散120秒→满屏停留30秒→支持DDC/CI则熄屏。\n任意鼠标/键盘输入立即取消流程并点亮，无需快捷键。\nRDP、其他程序模拟输入、音频输出/活动音频会话时不接管。\n模拟输入排除会持续到一次本机物理输入。\n\n参数：--idle-seconds N --ink-delay-seconds N --ink-grow-seconds N --ink-hold-seconds N\n--power-only 仅防休眠，无水墨动画/熄屏，不注入输入\n--paused --run-for-seconds N\n右键托盘：暂停/状态/退出。";
thread_local! {static APP:RefCell<Option<App>>=const{RefCell::new(None)};}
struct App {
    config: Config,
    enabled: bool,
    hwnd: HWND,
    taskbar_created: u32,
    session_blocked: bool,
    supply: ExternalPower,
    interactive: bool,
    phase: Phase,
    progress: f32,
    reason: String,
    power: PowerRequest,
    started: Instant,
    last_activity: u64,
    sequence: u64,
    last_stamp: u32,
    last_pulse: Option<Instant>,
    pulse_count: u64,
    warning: Option<String>,
    grass: Option<grass::Grass>,
    monitors: Option<monitor::Monitors>,
    off_attempted: bool,
    off_error: Option<String>,
    visual_error: Option<String>,
    audio: Option<audio::AudioMonitor>,
    audio_error: Option<String>,
    audio_playing: bool,
    audio_checked: u64,
}
impl App {
    fn restore_display(&mut self) -> Result<(), String> {
        if let Some(monitors) = self.monitors.as_mut() {
            monitors.restore()?;
            self.monitors = None;
            log("display restored by activity/state change");
        }
        Ok(())
    }
    fn stop(&mut self) {
        self.grass = None;
        if let Err(e) = self.restore_display() {
            self.warning = Some(e);
        }
        if let Err(e) = self.power.set(false) {
            self.warning = Some(e);
        }
    }
    fn reset_activity(&mut self, now: u64) {
        if self.phase.keeps_awake() {
            log("activity cancelled progressive flow");
        }
        self.last_activity = now;
        self.off_attempted = false;
        self.off_error = None;
        self.visual_error = None;
        self.stop();
    }
    fn tick(&mut self) {
        let now = unsafe { GetTickCount64() };
        let input = activity::snapshot();
        if input.sequence != self.sequence {
            self.sequence = input.sequence;
            self.reset_activity(input.last_ms);
        }
        let stamp = platform::last_input_stamp();
        if let Ok(stamp) = stamp.as_ref() {
            if *stamp != self.last_stamp && *stamp != input.own_tick {
                self.reset_activity(now);
            }
            self.last_stamp = *stamp;
        }
        self.supply = platform::external_power();
        self.interactive = !self.session_blocked && platform::interactive_desktop();
        if now.saturating_sub(self.audio_checked) >= 1000 {
            self.audio_checked = now;
            if let Some(audio) = self.audio.as_ref() {
                match audio.playing() {
                    Ok(playing) => {
                        self.audio_playing = playing;
                        self.audio_error = None;
                    }
                    Err(e) => self.audio_error = Some(e),
                }
            }
        }
        let remote = unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 };
        let reason = if !self.enabled {
            "已暂停".to_string()
        } else if self.supply != ExternalPower::Connected {
            "未接通外部电源，待机".into()
        } else if !self.interactive {
            "会话锁定/断开，待机".into()
        } else if remote {
            "远程桌面会话，不接管".into()
        } else if !input.healthy || stamp.is_err() {
            "输入监测不可用，不接管".into()
        } else if platform::input_held() {
            "鼠标按钮/修饰键按住，等待".into()
        } else if input.automation {
            "检测到模拟输入，不接管（本机物理输入后恢复）".into()
        } else if let Some(e) = self.audio_error.as_ref() {
            e.clone()
        } else if self.audio_playing {
            "音频输出/音频会话活动，不接管".into()
        } else {
            String::new()
        };
        let eligible = reason.is_empty();
        if !eligible {
            self.last_activity = now;
            self.off_attempted = false;
            self.off_error = None;
            self.visual_error = None;
        }
        let (mut next, progress) = config::phase(
            &self.config,
            now.saturating_sub(self.last_activity),
            eligible,
        );
        if self.reason != reason {
            log(&format!("guard={reason}"));
        }
        self.reason = reason;
        self.progress = progress;
        if next == Phase::Standby {
            self.stop();
        } else {
            if let Err(e) = self.power.set(true) {
                self.warning = Some(e);
                self.stop();
                next = Phase::Standby;
            }
            if next == Phase::Growing || next == Phase::FullGrass {
                if self.visual_error.is_none() && self.grass.is_none() {
                    match grass::Grass::new(self.config.grass_grow_seconds) {
                        Ok(g) => self.grass = Some(g),
                        Err(e) => {
                            log(&e);
                            self.visual_error = Some(e)
                        }
                    }
                }
                if let Some(g) = self.grass.as_mut() {
                    if let Err(e) = g.update(progress) {
                        self.visual_error = Some(e);
                        self.grass = None;
                        next = Phase::KeepAlive;
                    }
                } else {
                    next = Phase::KeepAlive;
                }
            }
            if next == Phase::ScreenOff {
                if let Some(g) = self.grass.as_mut()
                    && let Err(e) = g.update(1.0)
                {
                    self.visual_error = Some(e);
                }
                if self.visual_error.is_some() {
                    next = Phase::KeepAlive;
                } else if !self.off_attempted && self.monitors.is_none() {
                    self.off_attempted = true;
                    match self.try_screen_off(input.sequence) {
                        Ok(true) => {}
                        Ok(false) => {
                            self.reset_activity(now);
                            next = Phase::Standby;
                        }
                        Err(e) => {
                            log(&e);
                            self.off_error = Some(e);
                        }
                    }
                }
                // Keep the last ink frame visible during the DDC command so the
                // desktop does not flash bright just before hardware screen-off.
                self.grass = None;
            }
            // Own synthetic events never change the real-input timeline. No pulse after takeover.
            if next.keeps_awake()
                && !self.config.power_only
                && self.last_pulse.is_none_or(|p| p.elapsed().as_secs() >= 20)
                && activity::snapshot().sequence == self.sequence
                && !activity::snapshot().automation
                && platform::external_power() == ExternalPower::Connected
                && !self.audio_playing
            {
                self.last_pulse = Some(Instant::now());
                match platform::pulse() {
                    Ok(()) => {
                        self.pulse_count += 1;
                    }
                    Err(e) => {
                        log(&e);
                        self.warning = Some(e)
                    }
                }
            }
        }
        if self.phase != next {
            log(&format!("phase={next:?}"));
            self.phase = next;
        }
        self.tray(NIM_MODIFY);
        if self
            .config
            .run_for_seconds
            .is_some_and(|n| self.started.elapsed().as_secs() >= n)
        {
            unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
    fn try_screen_off(&mut self, sequence: u64) -> Result<bool, String> {
        let mut monitors = monitor::Monitors::discover()?;
        let media = self
            .audio
            .as_ref()
            .is_none_or(|a| a.playing().unwrap_or(true));
        if activity::snapshot().sequence != sequence
            || activity::snapshot().automation
            || media
            || platform::external_power() != ExternalPower::Connected
        {
            return Ok(false);
        }
        monitors.off()?;
        if activity::snapshot().sequence != sequence {
            monitors.restore()?;
            return Ok(false);
        }
        self.monitors = Some(monitors);
        log("progressive screen-off entered");
        Ok(true)
    }
    fn status(&self) -> String {
        let mode = if !self.reason.is_empty() {
            self.reason.clone()
        } else {
            match self.phase {
                Phase::Standby => "用户活动/等待空闲".into(),
                Phase::KeepAlive => "仅保活".into(),
                Phase::Growing => {
                    format!("四角水墨扩散 {:.0}%（透明度渐变）", self.progress * 100.)
                }
                Phase::FullGrass => "水墨已覆盖全屏，等待熄屏".into(),
                Phase::ScreenOff => {
                    if self.monitors.is_some() {
                        "熄屏保活（任意输入点亮）".into()
                    } else {
                        "仅保活（硬件熄屏不可用）".into()
                    }
                }
            }
        };
        format!(
            "{mode}\n保活事件：{}\n{}\n{}\n{}",
            self.pulse_count,
            self.off_error.as_deref().unwrap_or(""),
            self.visual_error.as_deref().unwrap_or(""),
            self.warning.as_deref().unwrap_or("")
        )
    }
    fn tray(&self, operation: u32) -> bool {
        unsafe {
            let mut data: NOTIFYICONDATAW = zeroed();
            data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = self.hwnd;
            data.uID = 1;
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = TRAY_MESSAGE;
            data.hIcon = LoadIconW(
                null_mut(),
                if self.warning.is_some() {
                    IDI_WARNING
                } else if self.phase.keeps_awake() {
                    IDI_INFORMATION
                } else {
                    IDI_APPLICATION
                },
            );
            let tip = format!(
                "Windows Awake：{}",
                self.status().lines().next().unwrap_or("")
            );
            for (dest, v) in data.szTip[..127].iter_mut().zip(tip.encode_utf16()) {
                *dest = v;
            }
            Shell_NotifyIconW(operation, &data) != 0
        }
    }
}
fn message(text: &str, error: bool) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("Windows Awake").as_ptr(),
            MB_OK
                | if error {
                    MB_ICONERROR
                } else {
                    MB_ICONINFORMATION
                },
        );
    }
}
fn popup(hwnd: HWND) {
    let snapshot = APP.with(|c| {
        c.borrow().as_ref().map(|a| {
            (
                a.enabled,
                a.config.power_only,
                a.status().lines().next().unwrap_or("").to_string(),
            )
        })
    });
    let Some((enabled, power_only, status)) = snapshot else {
        return;
    };
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, wide(&status).as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(
            menu,
            MF_STRING,
            TOGGLE as usize,
            wide(if enabled { "暂停" } else { "恢复" }).as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | if power_only { MF_CHECKED } else { 0 },
            POWER_ONLY as usize,
            wide("仅防休眠（无水墨/无输入注入）").as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, STATUS as usize, wide("查看状态").as_ptr());
        AppendMenuW(menu, MF_STRING, EXIT as usize, wide("退出").as_ptr());
        let mut p: POINT = zeroed();
        GetCursorPos(&mut p);
        SetForegroundWindow(hwnd);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            p.x,
            p.y,
            0,
            hwnd,
            null(),
        );
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        if selected != 0 {
            PostMessageW(hwnd, WM_COMMAND, selected as usize, 0);
        }
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        activity::INPUT_MESSAGE => {
            activity::acknowledge();
            APP.with(|c| {
                if let Some(a) = c.borrow_mut().as_mut() {
                    a.tick();
                }
            });
            0
        }
        WM_TIMER => {
            APP.with(|c| {
                if let Some(a) = c.borrow_mut().as_mut() {
                    a.tick();
                }
            });
            0
        }
        TRAY_MESSAGE => {
            match l as u32 {
                WM_RBUTTONUP | WM_CONTEXTMENU => popup(hwnd),
                WM_LBUTTONDBLCLK => unsafe {
                    PostMessageW(hwnd, WM_COMMAND, TOGGLE as usize, 0);
                },
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            let cmd = (w & 0xffff) as u32;
            if cmd == EXIT {
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            } else if cmd == STATUS {
                let text = APP.with(|c| c.borrow().as_ref().map(App::status));
                if let Some(text) = text {
                    message(&text, false)
                }
            } else {
                APP.with(|c| {
                    if let Some(a) = c.borrow_mut().as_mut() {
                        match cmd {
                            TOGGLE => a.enabled = !a.enabled,
                            POWER_ONLY => a.config.power_only = !a.config.power_only,
                            _ => return,
                        }
                        a.reset_activity(unsafe { GetTickCount64() });
                        a.tick();
                    }
                });
            }
            0
        }
        WM_POWERBROADCAST => {
            APP.with(|c| {
                if let Some(a) = c.borrow_mut().as_mut() {
                    if matches!(
                        w as u32,
                        PBT_APMSUSPEND | PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND
                    ) {
                        a.reset_activity(unsafe { GetTickCount64() });
                    }
                    a.tick();
                }
            });
            1
        }
        WM_DISPLAYCHANGE => {
            APP.with(|c| {
                if let Some(a) = c.borrow_mut().as_mut() {
                    a.reset_activity(unsafe { GetTickCount64() });
                    a.tick();
                }
            });
            0
        }
        WM_WTSSESSION_CHANGE => {
            APP.with(|c| {
                if let Some(a) = c.borrow_mut().as_mut() {
                    match w as u32 {
                        WTS_SESSION_LOCK
                        | WTS_CONSOLE_DISCONNECT
                        | WTS_REMOTE_DISCONNECT
                        | WTS_SESSION_LOGOFF => a.session_blocked = true,
                        WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT
                        | WTS_SESSION_LOGON => a.session_blocked = false,
                        _ => {}
                    }
                    a.reset_activity(unsafe { GetTickCount64() });
                    a.tick();
                }
            });
            0
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            unsafe {
                KillTimer(hwnd, 1);
                WTSUnRegisterSessionNotification(hwnd);
            }
            APP.with(|c| {
                if let Some(mut a) = c.borrow_mut().take() {
                    a.stop();
                    a.tray(NIM_DELETE);
                    log("stopped; state restored");
                }
            });
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION if w != 0 => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        _ => {
            APP.with(|c| {
                if let Some(a) = c.borrow().as_ref()
                    && a.taskbar_created != 0
                    && a.taskbar_created == msg
                {
                    a.tray(NIM_ADD);
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
    }
}
fn run(config: Config) -> Result<(), String> {
    let _instance = Instance::acquire()?;
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(null());
        let name = wide("WindowsAwakeHiddenWindow");
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            style: CS_DBLCLKS,
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return Err(format!("RegisterClassW: {}", GetLastError()));
        }
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("Windows Awake").as_ptr(),
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
        if hwnd.is_null() {
            return Err("创建后台窗口失败".into());
        }
        let watcher = match activity::Watcher::start(hwnd) {
            Ok(w) => w,
            Err(e) => {
                DestroyWindow(hwnd);
                return Err(e);
            }
        };
        let (audio, audio_error) = match audio::AudioMonitor::new() {
            Ok(a) => (Some(a), None),
            Err(e) => (None, Some(format!("音频状态检测不可用：{e}"))),
        };
        let app = App {
            enabled: !config.paused,
            config,
            hwnd,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            session_blocked: false,
            supply: ExternalPower::Unknown,
            interactive: false,
            phase: Phase::Standby,
            progress: 0.,
            reason: "等待空闲".into(),
            power: PowerRequest::default(),
            started: Instant::now(),
            last_activity: GetTickCount64(),
            sequence: activity::snapshot().sequence,
            last_stamp: platform::last_input_stamp().unwrap_or(0),
            last_pulse: None,
            pulse_count: 0,
            warning: None,
            grass: None,
            monitors: None,
            off_attempted: false,
            off_error: None,
            visual_error: None,
            audio,
            audio_error,
            audio_playing: false,
            audio_checked: 0,
        };
        APP.with(|c| *c.borrow_mut() = Some(app));
        if !APP.with(|c| c.borrow().as_ref().unwrap().tray(NIM_ADD)) {
            DestroyWindow(hwnd);
            return Err("创建托盘失败".into());
        }
        WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        if SetTimer(hwnd, 1, 100, None) == 0 {
            DestroyWindow(hwnd);
            return Err("创建计时器失败".into());
        }
        log("started progressive mode");
        APP.with(|c| c.borrow_mut().as_mut().unwrap().tick());
        let mut msg: MSG = zeroed();
        loop {
            let result = GetMessageW(&mut msg, null_mut(), 0, 0);
            if result == 0 {
                break;
            }
            if result < 0 {
                DestroyWindow(hwnd);
                return Err("消息循环失败".into());
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        drop(watcher);
    }
    Ok(())
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| {
        matches!(
            a.as_str(),
            "--grass-preview" | "--ink-preview" | "--ink-preview-frames"
        )
    }) {
        let result = args
            .get(1)
            .ok_or("缺少预览文件路径".to_string())
            .and_then(|path| {
                if args[0] == "--ink-preview-frames" {
                    grass::preview_frames(path)
                } else {
                    grass::preview(path)
                }
            });
        if let Err(e) = result {
            message(&e, true);
            std::process::exit(2);
        }
        return;
    }
    match Config::parse(args) {
        Ok(Some(c)) => {
            if let Err(e) = run(c) {
                log(&e);
                message(&e, true);
                std::process::exit(1);
            }
        }
        Ok(None) => message(HELP, false),
        Err(e) => {
            message(&format!("{e}\n\n{HELP}"), true);
            std::process::exit(2);
        }
    }
}
