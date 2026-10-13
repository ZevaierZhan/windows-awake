# Windows Awake

[![CI](https://github.com/ZevaierZhan/windows-awake/actions/workflows/ci.yml/badge.svg)](https://github.com/ZevaierZhan/windows-awake/actions/workflows/ci.yml)

[下载最新版 EXE](https://github.com/ZevaierZhan/windows-awake/releases/latest/download/WindowsAwake.exe) · [发布记录](https://github.com/ZevaierZhan/windows-awake/releases)

Rust 实现的 Windows 10/11 x64 托盘保活工具，具有渐进式草地覆盖和 DDC/CI 硬件熄屏。无需 Python 或管理员权限。

## 默认流程

只有接通外部电源、当前为本机交互会话、无人输入且没有音频活动时，才启动以下流程：

| 连续空闲时间 | 行为 |
| --- | --- |
| 0～30 秒 | 待机，不保活、不覆盖屏幕 |
| 30～60 秒 | 仅保活：保持系统/屏幕唤醒，定期发送零位移鼠标事件 |
| 60～180 秒 | 草地从每块屏幕的四个角向中心生长，不透明度逐渐提高 |
| 180～210 秒 | 草地已长满，再停留 30 秒 |
| 210 秒以后 | 支持 DDC/CI 的全部显示器硬件熄屏，继续保活 |

任意鼠标/键盘活动都会撤掉草地、发送显示器恢复命令、释放保活请求，并重新等待完整的空闲时间。鼠标按钮或修饰键按住期间也不会接管。**无需特殊快捷键。**显示器实际点亮速度取决于硬件；若硬件不响应恢复命令，请使用显示器实体电源键。

DDC/CI 不支持、检测失败或关屏失败时，自动回退为普通保活，不会在本轮空闲中反复尝试。草地窗口无法安全建立或截图排除不可用时，跳过草地和熄屏，继续普通保活。

## 不接管输入、远程控制和音频播放

- RDP 会话不启动保活/长草/熄屏。
- 任意外部输入，无论本机还是远程，都会重置流程。
- 检测到其他程序注入的鼠标/键盘事件，会持续暂停到下一次本机物理输入，避免远控或 computer use 暂时不操作时重新接管。没有按进程名称猜测接管状态。
- 检测所有活动音频输出设备的峰值和音频会话状态。正在输出声音，或存在运行中的音频流（包括静音流/静音会议）时，不启动流程；已经启动的流程会被取消。只读取状态，不录音、不访问麦克风。
- 草地窗口不获取焦点、鼠标穿透，使用系统支持的截图排除接口减少对远程/自动化画面的影响；该接口不是对所有截图方式的保证。

Windows 没有通用的“computer use 已接管”标志。未标记为注入的虚拟 HID/硬件模拟输入无法可靠区分本机用户与自动化：活跃输入仍会重置流程，但不能保证识别这类工具的长时间停顿。完全没有音频流的静音视频也不能仅通过音频状态识别。

输入监测使用独立线程上的鼠标/键盘低级钩子，并始终转发事件，不拦截输入。钩子只观察时间、注入标志和本程序标记，不保存按键值、输入内容或鼠标位置。`GetLastInputInfo` 作为活动时间回退；本程序保活事件具有独立标记和显式时间戳，**不会重置草地计时或触发自动化排除**。输入/音频监测不可用时，停止接管。

## 运行和参数

双击 `WindowsAwake.exe` 启动。右键托盘可暂停/恢复、查看阶段与排除原因或退出。拔电、锁屏、会话断开、暂停或退出时取消流程并尝试恢复显示器；重新接电/恢复后重新等待空闲。不会修改电源计划、锁屏策略、注册表或开机启动项。

```powershell
.\WindowsAwake.exe
.\WindowsAwake.exe --idle-seconds 60 --grass-delay-seconds 30 --grass-grow-seconds 180 --grass-hold-seconds 60
.\WindowsAwake.exe --power-only
.\WindowsAwake.exe --paused
.\WindowsAwake.exe --help
```

各阶段时间为 1～3600 秒。`--power-only` 在空闲后只申请电源保活，不注入输入、不长草、不熄屏；同样服从输入、音频及会话排除。菜单设置只对本次运行生效。

日志：`%LOCALAPPDATA%\WindowsAwake\activity.log`，记录阶段、排除原因、保活/显示器错误，不记录输入或音频内容。

## 构建、测试和发布

需要 Rust stable、Visual Studio C++ Build Tools 和 Windows SDK。

```powershell
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
.\scripts\build.ps1
```

输出：`dist\WindowsAwake.exe`。常规测试验证阶段边界、满屏等待、活动/排除重置、四角生长、不透明度单调增加和输入来源分类。真实桌面、输入或显示器测试标为 ignored，不会在 GitHub runner 上执行；不要一次运行全部 ignored 测试。

```powershell
# 检查输入钩子/本程序时间戳；会注入零位移和一次 F24 按下/释放，仅用于诊断
cargo test --locked live_hook_and_own_timing -- --ignored --nocapture
# 真机 DDC/CI 检查：短暂关屏并恢复，请只在有人监督时运行
cargo test --locked ddc_off_with_zero_input -- --ignored --nocapture
# 从实际 GDI 草地绘制器生成四阶段 BMP 预览，不操作桌面
.\WindowsAwake.exe --grass-preview preview.bmp
```

主分支及 PR 运行 CI。在 Actions 选择 **Release Windows EXE → Run workflow**，填写与 `Cargo.toml` 一致的版本标签，或推送 `v*` 标签，即可发布 EXE、ZIP 和 SHA-256 校验文件。

```powershell
gh workflow run release.yml --ref main -f tag=v0.2.0
```

本机测试验证了输入钩子能看到外部键盘注入；零位移事件可能刷新系统空闲时间而不进入低级钩子，因此单独识别本程序事件时间戳。实机测试期间已有活动音频会话，应用保持待机，没有为了验证完整动画而强制绕过媒体排除。草地视觉使用同一 GDI 绘制器进行离屏检查。

DDC/CI 取决于显示器，部分内置屏/扩展坞不支持。公司强制锁屏策略、管理员前台窗口或忽略模拟输入的策略可能影响防锁屏效果；不阻止手动 Win+L、注销/关盖，也不会自动解锁。

## 接口参考

- [Windows 输入钩子与低级鼠标钩子](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc)
- [透明分层窗口与鼠标穿透](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features)
- [截图排除接口](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity)
- [音频会话活动状态](https://learn.microsoft.com/en-us/windows/win32/api/audiopolicy/nf-audiopolicy-iaudiosessioncontrol-getstate)
- [DDC/CI 电源控制](https://learn.microsoft.com/en-us/windows/win32/api/lowlevelmonitorconfigurationapi/nf-lowlevelmonitorconfigurationapi-setvcpfeature)
- [系统电源保活](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate)
