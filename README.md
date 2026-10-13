# Windows Awake

[![CI](https://github.com/ZevaierZhan/windows-awake/actions/workflows/ci.yml/badge.svg)](https://github.com/ZevaierZhan/windows-awake/actions/workflows/ci.yml)

[下载最新版 EXE](https://github.com/ZevaierZhan/windows-awake/releases/latest/download/WindowsAwake.exe) · [发布记录](https://github.com/ZevaierZhan/windows-awake/releases)

Rust 实现的 Windows 10/11 x64 托盘保活工具。双击 `dist/WindowsAwake.exe` 启动，无控制台窗口，无需 Python、管理员权限或安装。源码和发布包可在本仓库获取。

## 使用

只有 Windows 明确报告已接通外部电源时才申请保持系统和屏幕唤醒；每秒读取当前会话的最后输入时间，连续空闲 30 秒后发送一个**相对零位移鼠标事件**。真实输入或远程工具注入的输入会更新最后输入时间，延后下一次保活。程序自己的保活事件也会更新计时，所以连续无人操作时约每 30 秒保活一次。

程序启动及每秒检查供电状态，并响应 Windows 电源状态变化通知。拔掉电源时停止输入注入并释放电源请求，程序保留在托盘待机；插回电源后自动恢复。供电状态未知或查询失败也暂停保活。此规则同时适用于普通模式和“仅防休眠”模式；手动暂停后，插电不会覆盖暂停状态。托盘状态会显示电池供电/供电状态未知。

系统托盘图标右键菜单：

- 暂停/恢复保活：暂停同时释放电源请求并停止输入注入。
- 仅防休眠（无输入注入）：只保持屏幕和系统唤醒；需要与 computer use 严格隔离输入时使用。
- 当前模式：菜单顶部和托盘悬浮提示自动显示“硬件熄屏保活”或“普通保活”；默认无需选择模式。
- 查看状态：查看空闲阈值、已接受事件数和错误。
- 退出：移除图标、释放保活请求，恢复 Windows 正常空闲行为。

双击托盘图标也可暂停/恢复。图标可能收纳在托盘的隐藏图标区域。同一 Windows 会话只允许一个实例。启动不会修改电源计划、锁屏策略、注册表或开机启动项。

```powershell
.\WindowsAwake.exe
.\WindowsAwake.exe --idle-seconds 60
.\WindowsAwake.exe --power-only
.\WindowsAwake.exe --paused
.\WindowsAwake.exe --idle-seconds 5 --run-for-seconds 10
.\WindowsAwake.exe --help
```

空闲阈值为 5～3600 秒。应小于电脑的自动锁屏超时；默认 30 秒。菜单设置仅对本次运行生效，重启恢复命令行选项/默认值。

## 为什么不会拉走鼠标

使用 `SendInput` 的 `MOUSEEVENTF_MOVE`，`dx=0, dy=0`，没有绝对坐标、光标复位、点击、滚轮或键盘输入。不会保存旧坐标再恢复，因此不覆盖远程桌面或 computer use 正在改变的位置。保活事件标记 `dwExtraInfo=0x41574B45`，便于诊断工具辨识。

不安装键盘/鼠标钩子，不采集按键或坐标，只用 `GetLastInputInfo` 读取最后一次输入的时间。注入前再检查一次空闲和当前桌面。休眠保活通过 `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)` 实现，并在暂停、退出或非交互会话中释放。

**零位移仍是一个鼠标事件**。监听所有注入事件的应用或自动化框架仍可能观察到它；不能保证与任何第三方工具都完全兼容。需要完全无输入事件时，选择“仅防休眠”，该模式不防止基于输入空闲时间的自动锁屏。程序无法通用识别所有远程控制/computer use 工具的工作状态，因此不采用进程名称猜测。

## 防锁屏的范围

本机实测零位移事件会刷新最后输入时间且不改变光标位置，可用于通常依赖输入空闲时间的屏保/自动锁屏。电源 API 本身不会阻止屏保。

- 不能阻止手动 Win+L、显式注销/休眠、关盖等用户操作，也不会自动解锁。
- 公司强制锁屏策略、动态锁、忽略模拟输入的策略可能仍会锁屏。
- Windows UIPI 可能阻止向管理员前台窗口注入；程序显示警告并记录失败，不自动申请提升权限或改为真实移动。
- 程序仅在当前会话连接且输入桌面为 `Default` 时保活。锁屏、RDP 断开、切换到安全桌面后停止注入并释放电源请求；重新连接/解锁后恢复。启动即锁屏、会话查询失败时也不会注入。
- 只检测所在 Windows 会话的输入。若通过 RDP 使用另一会话，请在目标会话运行程序。
- 本机输入刷新实测不等于已经验证了实际公司锁屏策略、所有远程桌面或 computer use 软件。

日志位置：`%LOCALAPPDATA%\WindowsAwake\activity.log`，记录启动、暂停、模式变化、保活结果和错误，不记录输入内容。单文件超过 1 MiB 时轮换，保留一个历史文件。托盘黄色警告表示遇到保活错误，详情见状态菜单。

## 构建和验证

需要 Rust stable 和 Visual Studio C++ Build Tools（MSVC Windows SDK）。项目 `rust-toolchain.toml` 只为本项目指定 stable，不修改全局默认工具链。

```powershell
cargo test --locked
cargo test --locked zero_displacement_refreshes_idle_without_moving_cursor -- --ignored --nocapture
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
.\scripts\build.ps1
```

常规测试验证自动检测一次、兼容性回退、用户点亮后不立即重关屏，以及接电/断电/状态未知/重新接电的保活门控、空闲阈值、暂停/模式/会话门控、49.7 天计时回绕和参数边界。标记 ignored 的实机测试使用生产代码发送一个零位移事件，验证最后输入时间刷新和鼠标坐标不变；运行时请保持鼠标静止并确保桌面已解锁。它不会设置锁屏策略或锁定电脑。

Release EXE 位于 `target\release\windows-awake.exe`；构建脚本将其复制到 `dist\WindowsAwake.exe`。发布程序不依赖外部 Rust DLL。

## 参考

同事 ZIP 中的 Python 示例分别持续设置鼠标绝对坐标和持续按住 F24，本项目仅将它们作为需求参考，没有复用持续移动或按键保持逻辑。

- [SetThreadExecutionState](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setthreadexecutionstate)
- [SendInput 与 UIPI](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
- [GetLastInputInfo 的会话范围](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getlastinputinfo)
- [MOUSEINPUT 相对位移](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput)

- [GetSystemPowerStatus 供电状态](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getsystempowerstatus)



## 熄屏并继续保活

接通外部电源并启用保活后，程序自动检测全部显示器。若全部支持 DDC/CI 硬件熄屏，就自动熄屏并继续保活；若不支持、查询/熄屏失败或点亮快捷键被占用，则自动使用普通保活。无需手动选择模式，不弹出兼容性错误对话框。托盘菜单顶部、悬浮提示和状态窗口显示当前模式及普通保活的原因。该功能直接使用显示器的硬件电源控制（VCP D6=04），持续保持 Windows 系统/逻辑显示唤醒以及空闲鼠标保活，不切换到“仅防休眠”模式。不会设置黑色覆盖窗口，不更改亮度，不关闭防自动锁屏的输入保活。

本次保活期间只检测一次，不会每秒重复探测硬件或反复关屏。拔电/暂停后重新接电/恢复，会自动重新检测。

**点亮：按 Ctrl+Alt+F10**，或在托盘选择“点亮显示器”。这个快捷键仅在硬件熄屏期间注册，未安装键盘钩子；若快捷键已被其他程序占用，则取消熄屏。普通鼠标/键盘输入不保证会点亮 DDC/CI 关闭的显示器，因此请记住点亮快捷键。主动点亮后保持普通保活，不会在下一秒重新熄屏；直到下一次暂停/恢复或拔电/接电才重新自动检测。

暂停、退出、拔掉外部电源、会话锁定/断开或切换到“仅防休眠”时会尝试恢复显示器原电源状态。恢复命令失败时记录错误；必要时使用显示器实体电源键。显示器断开连接或程序被强制结束时不能保证自动点亮。

程序逐个检查全部显示器：必须能读取 DDC/CI 能力字符串，明确声明支持硬件关闭（D6=04），并确认目前已开启。任何显示器不支持或查询失败都会自动退回普通保活，并在状态中显示原因。常见笔记本内置屏、部分扩展坞或远程虚拟显示器可能不支持该接口。此功能不会绕过前文描述的公司锁屏策略/UIPI 限制。

### 实机结果及验证

Windows 软件熄屏测试：零位移鼠标保活会把 Windows 显示状态从关闭恢复为开启，故不采用软件熄屏加反复关屏的方式。

DDC/CI 硬件测试：本机显示器在保活前后均读回电源状态 4（关闭），且 Windows 最后输入时间得到刷新。测试结束发送恢复原电源状态命令；显示器控制器刚恢复时可能暂时无法响应状态读取。接口读回结果不能代替肉眼检查实际背光，其他设备也需要实机验证。

以下为显式硬件诊断，不纳入常规测试；不要一次执行全部 ignored 测试：

```powershell
# 会短暂关闭屏幕，随后自动恢复；运行时请勿操作鼠标/键盘
cargo test --locked screen_off_with_zero_input -- --ignored --nocapture
# 只读查询 DDC/CI 电源控制
cargo test --locked inspect_ddc_power_support -- --ignored --nocapture
# 验证硬件关屏后鼠标保活不改回开启状态，再发送点亮命令
cargo test --locked ddc_off_with_zero_input -- --ignored --nocapture
```

软件屏幕探测的 `test ok` 仅表示探测程序完成，不代表屏幕保持关闭。DDC/CI 测试另外断言电源状态保持 4、输入时间刷新；不支持的设备会输出 unavailable，不代表支持。

- [微软 DDC/CI 显示器控制接口及兼容性说明](https://learn.microsoft.com/en-us/windows/win32/api/lowlevelmonitorconfigurationapi/nf-lowlevelmonitorconfigurationapi-setvcpfeature)


## GitHub Actions 发布

主分支推送或 Pull Request 会运行 Windows CI。Release 工作流会检查格式、测试、Clippy，然后编译 EXE，生成 ZIP 和 SHA-256 校验文件，并创建公开的 GitHub Release。

在 Actions 中选择 **Release Windows EXE → Run workflow**，填写与 `Cargo.toml` 一致的版本标签；也可以推送 `v*` 标签触发发布。命令行触发示例：

```powershell
gh workflow run release.yml --ref main -f tag=v0.1.0
```

常规测试不会注入输入或关屏；需要真实桌面/显示器的测试显式标为 ignored，不会在云端 runner 上自动执行。
