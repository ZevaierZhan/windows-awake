#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod config;
#[cfg(test)]
mod display_probe;
mod monitor;
mod platform;
#[cfg(test)]
mod probe;

use config::{AutoDisplay, Config, ExternalPower, should_keep_awake, should_pulse};
use platform::{Instance, PowerRequest, log, wide};
use std::{
    cell::RefCell,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    time::Instant,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, RemoteDesktop::*},
    UI::{Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const TRAY_MESSAGE: u32 = WM_APP + 1;
const TOGGLE: u32 = 100;
const POWER_ONLY: u32 = 101;
const EXIT: u32 = 102;
const STATUS: u32 = 103;
const DISPLAY_TOGGLE: u32 = 104;
const WAKE_HOTKEY: i32 = 1;
const HELP: &str = "Windows Awake\n\n默认：仅接通外部电源时保活；空闲 30 秒后发送零位移鼠标事件。\n拔掉电源自动暂停，插回自动恢复；手动暂停不会自动恢复。\n自动检测显示器：支持 DDC/CI 硬件熄屏则自动熄屏并保活，否则普通保活。\n托盘显示当前模式，无需手动设置。\n右键托盘：暂停 / 仅防休眠 / 状态 / 退出。\n硬件熄屏后按 Ctrl+Alt+F10 点亮；需要显示器支持 DDC/CI。\n双击托盘：暂停或恢复。\n\n参数：\n--idle-seconds 5..3600\n--power-only  仅申请电源保活，不注入任何输入\n--paused  暂停启动\n--run-for-seconds N  N 秒后自动退出\n\n不改变电源设置，不自动启动，不阻止手动锁屏。\n管理员前台窗口、公司策略、RDP 断开可能限制防锁屏效果。\n日志：%LOCALAPPDATA%\\WindowsAwake\\activity.log";
thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }
struct App {
    config: Config,
    enabled: bool,
    session_blocked: bool,
    interactive: bool,
    supply: ExternalPower,
    power: PowerRequest,
    started: Instant,
    last_attempt: Option<Instant>,
    pulse_count: u64,
    warning: Option<String>,
    taskbar_created: u32,
    hwnd: HWND,
    monitors: Option<monitor::Monitors>,
    display_mode: AutoDisplay,
}
impl App {
    fn restore_display(&mut self) -> Result<(), String> {
        if let Some(monitors) = self.monitors.as_mut() {
            monitors.restore()?;
            self.monitors = None;
            unsafe {
                UnregisterHotKey(self.hwnd, WAKE_HOTKEY);
            }
            log("DDC display restored");
        }
        Ok(())
    }
    fn start_display_off(&mut self) -> Result<(), String> {
        if !should_keep_awake(
            self.enabled,
            platform::interactive_desktop(),
            platform::external_power(),
        ) {
            return Err("请先接通外部电源、解锁会话并恢复保活，再使用硬件熄屏。".into());
        }
        let mut monitors = monitor::Monitors::discover()?;
        if unsafe {
            RegisterHotKey(
                self.hwnd,
                WAKE_HOTKEY,
                MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                VK_F10 as u32,
            )
        } == 0
        {
            return Err("无法注册 Ctrl+Alt+F10 点亮快捷键，未关闭显示器。".into());
        }
        if let Err(error) = monitors.off() {
            unsafe {
                UnregisterHotKey(self.hwnd, WAKE_HOTKEY);
            }
            return Err(error);
        }
        self.monitors = Some(monitors);
        log("DDC display off; idle input keepalive remains enabled");
        Ok(())
    }
    fn wake_display(&mut self) -> Result<(), String> {
        self.restore_display()?;
        self.display_mode = AutoDisplay::UserAwake;
        log("display awake by user; automatic screen-off deferred until next active period");
        Ok(())
    }
    fn tick(&mut self) {
        let supply = platform::external_power();
        if supply != self.supply {
            log(&format!("external_power={supply:?}"));
        }
        self.supply = supply;
        let interactive = !self.session_blocked && platform::interactive_desktop();
        if interactive != self.interactive {
            log(&format!("interactive={interactive}"));
        }
        self.interactive = interactive;
        let active = should_keep_awake(self.enabled, self.interactive, self.supply);
        let power_warning = self.power.set(active).err();
        let monitor_warning = if !active {
            self.display_mode = AutoDisplay::Pending;
            self.restore_display().err()
        } else {
            None
        };
        if self
            .display_mode
            .should_probe(active, self.config.power_only)
            && self.monitors.is_none()
            && power_warning.is_none()
        {
            self.display_mode = match self.start_display_off() {
                Ok(()) => AutoDisplay::HardwareOff,
                Err(reason) => {
                    log(&format!("automatic display fallback: {reason}"));
                    AutoDisplay::Fallback(reason)
                }
            };
        }
        let mut warning = if active && !self.config.power_only {
            self.warning.clone()
        } else {
            None
        };
        if active && !self.config.power_only {
            match platform::last_input_idle() {
                Ok(idle) => {
                    // Recheck activity before injection; failed attempts are rate limited.
                    let retry_ready = self
                        .last_attempt
                        .is_none_or(|at| at.elapsed().as_secs() >= self.config.idle_seconds as u64);
                    if retry_ready
                        && should_pulse(
                            active,
                            self.config.power_only,
                            self.interactive,
                            idle,
                            self.config.idle_seconds,
                        )
                        && platform::last_input_idle()
                            .is_ok_and(|fresh| fresh >= self.config.idle_seconds * 1000)
                        && platform::interactive_desktop()
                        && platform::external_power() == ExternalPower::Connected
                    {
                        self.last_attempt = Some(Instant::now());
                        match platform::pulse() {
                            Ok(()) => {
                                warning = None;
                                self.pulse_count += 1;
                                log(&format!(
                                    "pulse accepted count={} idle_ms={idle}",
                                    self.pulse_count
                                ));
                            }
                            Err(err) => warning = Some(err),
                        }
                    }
                }
                Err(err) => warning = Some(err),
            }
        }
        if power_warning.is_some() {
            warning = power_warning;
        }
        if monitor_warning.is_some() {
            warning = monitor_warning;
        }
        if warning != self.warning {
            if let Some(ref err) = warning {
                log(err);
            }
            self.warning = warning;
        }
        self.tray(NIM_MODIFY);
        if self
            .config
            .run_for_seconds
            .is_some_and(|seconds| self.started.elapsed().as_secs() >= seconds)
        {
            unsafe {
                PostMessageW(self.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
    fn status(&self) -> String {
        let state = if !self.enabled {
            "已暂停"
        } else if self.supply == ExternalPower::Battery {
            "电池供电，保活已自动暂停"
        } else if self.supply == ExternalPower::Unknown {
            "供电状态未知，保活已自动暂停"
        } else if !self.interactive {
            "会话锁定/断开，保活已挂起"
        } else if self.config.power_only {
            "仅防休眠（无输入注入）"
        } else if self.monitors.is_some() {
            "硬件熄屏保活中（Ctrl+Alt+F10 点亮）"
        } else if matches!(self.display_mode, AutoDisplay::UserAwake) {
            "普通保活（已由用户点亮屏幕）"
        } else {
            "普通保活（自动熄屏不可用）"
        };
        let display_detail = match &self.display_mode {
            AutoDisplay::Fallback(reason) => format!("自动选择普通保活：{reason}"),
            _ => String::new(),
        };
        format!(
            "{state}\n{display_detail}\n空闲阈值：{} 秒\n已接受保活事件：{}\n{}",
            self.config.idle_seconds,
            self.pulse_count,
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
            let icon = if self.warning.is_some() {
                IDI_WARNING
            } else if should_keep_awake(self.enabled, self.interactive, self.supply) {
                IDI_INFORMATION
            } else {
                IDI_APPLICATION
            };
            data.hIcon = LoadIconW(null_mut(), icon);
            let tip = format!("Windows Awake: {}", self.status().replace('\n', " / "));
            for (dest, value) in data.szTip[..127].iter_mut().zip(tip.encode_utf16()) {
                *dest = value;
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
    // Never hold a RefCell borrow across TrackPopupMenu's nested message loop.
    let snapshot = APP.with(|cell| {
        cell.borrow().as_ref().map(|app| {
            (
                app.enabled,
                app.config.power_only,
                app.monitors.is_some(),
                app.status().lines().next().unwrap_or("").to_string(),
            )
        })
    });
    let Some((enabled, power_only, display_off, current_mode)) = snapshot else {
        return;
    };
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        AppendMenuW(
            menu,
            MF_STRING | MF_GRAYED,
            0,
            wide(&format!("当前模式：{current_mode}")).as_ptr(),
        );
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(
            menu,
            MF_STRING,
            TOGGLE as usize,
            wide(if enabled {
                "暂停保活"
            } else {
                "恢复保活"
            })
            .as_ptr(),
        );
        AppendMenuW(
            menu,
            MF_STRING | if power_only { MF_CHECKED } else { 0 },
            POWER_ONLY as usize,
            wide("仅防休眠（无输入注入）").as_ptr(),
        );
        AppendMenuW(menu, MF_STRING, STATUS as usize, wide("查看状态").as_ptr());
        if display_off {
            AppendMenuW(
                menu,
                MF_STRING,
                DISPLAY_TOGGLE as usize,
                wide("点亮显示器（Ctrl+Alt+F10）").as_ptr(),
            );
        }
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, EXIT as usize, wide("退出").as_ptr());
        let mut point: POINT = zeroed();
        GetCursorPos(&mut point);
        SetForegroundWindow(hwnd);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
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
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Win32 invokes this on the message-loop thread that owns APP.
    match msg {
        WM_TIMER => {
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    app.tick();
                }
            });
            0
        }
        TRAY_MESSAGE => {
            match lparam as u32 {
                WM_RBUTTONUP | WM_CONTEXTMENU => popup(hwnd),
                WM_LBUTTONDBLCLK => unsafe {
                    PostMessageW(hwnd, WM_COMMAND, TOGGLE as usize, 0);
                },
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            let command = (wparam & 0xffff) as u32;
            if command == EXIT {
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            } else if command == DISPLAY_TOGGLE {
                let result = APP.with(|cell| {
                    cell.borrow_mut().as_mut().map(|app| {
                        let result = app.wake_display();
                        app.tick();
                        result
                    })
                });
                if let Some(Err(err)) = result {
                    log(&err);
                    message(&err, true);
                }
            } else if command == STATUS {
                let text = APP.with(|cell| cell.borrow().as_ref().map(App::status));
                if let Some(text) = text {
                    message(&text, false);
                }
            } else {
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        match command {
                            TOGGLE => {
                                app.enabled = !app.enabled;
                                log(if app.enabled { "resumed" } else { "paused" });
                            }
                            POWER_ONLY => {
                                if let Err(err) = app.restore_display() {
                                    app.warning = Some(err);
                                    return;
                                }
                                app.config.power_only = !app.config.power_only;
                                app.display_mode = AutoDisplay::Pending;
                                log(&format!("power_only={}", app.config.power_only));
                            }
                            _ => return,
                        }
                        app.tick();
                    }
                });
            }
            0
        }
        WM_HOTKEY if wparam == WAKE_HOTKEY as usize => {
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    if let Err(err) = app.wake_display() {
                        log(&err);
                        app.warning = Some(err);
                    }
                    app.tick();
                }
            });
            0
        }
        WM_POWERBROADCAST if wparam as u32 == PBT_APMPOWERSTATUSCHANGE => {
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    app.tick();
                }
            });
            1
        }
        WM_WTSSESSION_CHANGE => {
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    match wparam as u32 {
                        WTS_SESSION_LOCK
                        | WTS_CONSOLE_DISCONNECT
                        | WTS_REMOTE_DISCONNECT
                        | WTS_SESSION_LOGOFF => app.session_blocked = true,
                        WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT
                        | WTS_SESSION_LOGON => app.session_blocked = false,
                        _ => {}
                    }
                    app.tick();
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
            APP.with(|cell| {
                if let Some(mut app) = cell.borrow_mut().take() {
                    if let Err(err) = app.restore_display() {
                        log(&err);
                    }
                    unsafe {
                        UnregisterHotKey(hwnd, WAKE_HOTKEY);
                    }
                    app.tray(NIM_DELETE);
                    log("stopped; power request released");
                }
            });
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION if wparam != 0 => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        _ => {
            APP.with(|cell| {
                if let Some(app) = cell.borrow().as_ref()
                    && app.taskbar_created != 0
                    && msg == app.taskbar_created
                {
                    app.tray(NIM_ADD);
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}
fn run(config: Config) -> Result<(), String> {
    let _instance = Instance::acquire()?;
    unsafe {
        let instance = GetModuleHandleW(null());
        let name = wide("WindowsAwakeHiddenWindow");
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            style: CS_DBLCLKS,
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return Err(format!("RegisterClassW: {}", GetLastError()));
        }
        // Hidden top-level window receives Explorer restart and session notifications.
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
            return Err(format!("CreateWindowExW: {}", GetLastError()));
        }
        let app = App {
            monitors: None,
            display_mode: AutoDisplay::Pending,
            enabled: !config.paused,
            supply: ExternalPower::Unknown,
            config,
            session_blocked: false,
            interactive: false,
            power: PowerRequest::default(),
            started: Instant::now(),
            last_attempt: None,
            pulse_count: 0,
            warning: None,
            taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            hwnd,
        };
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        let tray_ok = APP.with(|cell| cell.borrow().as_ref().unwrap().tray(NIM_ADD));
        if !tray_ok {
            DestroyWindow(hwnd);
            return Err("创建托盘图标失败，程序已停止。".into());
        }
        if WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
            log("WTS notification unavailable; desktop/session polling remains enabled");
        }
        if SetTimer(hwnd, 1, 1000, None) == 0 {
            DestroyWindow(hwnd);
            return Err("创建计时器失败".into());
        }
        log("started");
        APP.with(|cell| cell.borrow_mut().as_mut().unwrap().tick());
        let mut msg: MSG = zeroed();
        loop {
            let result = GetMessageW(&mut msg, null_mut(), 0, 0);
            if result == 0 {
                break;
            }
            if result == -1 {
                DestroyWindow(hwnd);
                return Err(format!("GetMessageW: {}", GetLastError()));
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
fn main() {
    match Config::parse(std::env::args().skip(1)) {
        Ok(Some(config)) => {
            if let Err(err) = run(config) {
                log(&err);
                message(&err, true);
                std::process::exit(1);
            }
        }
        Ok(None) => message(HELP, false),
        Err(err) => {
            message(&format!("{err}\n\n{HELP}"), true);
            std::process::exit(2);
        }
    }
}
