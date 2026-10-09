# Windows 原生验证入口

已通过 WSL2 调用 Windows Rust 1.99.0：x64 workspace 432 个测试、生产 Win32 符号链接与 clippy 通过；SDK x64/x86 oracle 均通过。结果和版本见 [本机验证记录](validation/windows-2026-10-09.json)。项目固定 Rust 1.97.1 的 Windows 验证、i686 Rust 测试与实际输入验收仍待执行。生产代码与 unit-test 的 Win32 API 替换分开验证。

## SDK 与编译

使用 Windows x64 主机，安装 Visual Studio C++ Build Tools 与 Windows SDK。`tools/verify_windows_abi.py` 使用 [Microsoft vswhere](https://github.com/microsoft/vswhere/wiki/Find-VC) 定位工具，不假定 VS edition/version。执行：

```text
python tools/verify_windows_abi.py --arch x64
python tools/verify_windows_abi.py --arch x86
rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc --toolchain 1.97.1
cargo test --locked --workspace --target x86_64-pc-windows-msvc
cargo test --locked --workspace --target i686-pc-windows-msvc
```

SDK oracle 为 `tools/windows_input_abi.c`，只编译 `/Zs`，不链接/执行；验证实际 SDK 的 INPUT/KEYBDINPUT/MOUSEINPUT/HARDWAREINPUT/KBDLLHOOKSTRUCT/MSG/USEROBJECTFLAGS/TOKEN_MANDATORY_LABEL 大小、offset/alignment、固定目标 pointer width、flags/消息/wait 常量，以及 WINAPI 原型。`_WIN32_WINNT/WINVER=0x0a00` 固定 Windows 10/11 接口层：QS_ALLINPUT 包含 TOUCH/POINTER，值为 0x1cff，不沿用旧 0x04ff。

Rust `native_link` integration test 通过正常 crate 依赖构建生产模块，保留 Hook/SendInput/MsgWait/desktop-token sampling 等函数地址以验证链接；只构造固定内存 buffer，不调用安装/注入。Rust native 布局断言与 SDK oracle 在同一个 x86/x64 CI job 中运行，任一 mismatch 阻止通过。workspace unit tests 在 Windows 也替换 Hook API，不进行全局捕获。

`.github/workflows/rust.yml` 已配置两种 MSVC 目标。工作流存在不等于已经执行或通过；真实结果须记录 runner、SDK/MSVC/Rust 版本、target 与 job URL。此前仅使用 Linux rust-std，隔离到 `/tmp` 的 Windows target 下载因 DNS 失败。现已发现并实际使用宿主 Windows 的 Rust/MSVC/SDK：Windows 10 build 19045、Rust 1.99.0、MSVC 19.44.35229.0 / tools 14.44.35207、SDK 10.0.26100.0；本机通过不代表 CI 已运行。

## 真机门槛仍未完成

编译/链接/SDK 检查均不能证明输入拦截或应用接收。待完整可执行会话、neutral/碰撞/重复/紧急退出/监督接入后，按 D08 记录 Windows 11 x64、普通/管理员程序、中文布局、特殊键、Injected、睡眠/安全桌面/RDP，以及部分发送后的实际恢复。不能用上述无需安装的测试替代这组验收。

来源：[Microsoft WinUser.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/WinUser.h)、[INPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-input)、[KBDLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct)、[MSG](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msg)、[MsgWaitForMultipleObjectsEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)。

只读采样新增 sample_input_state 的 WinSta0/UOI_IO/desktop access、foreground token integrity 与无 thread impersonation 检查；此路径依赖 user32/kernel32/advapi32，布局和 symbol retention 已加入验证入口。IdleObserved 不是 neutral/capture-ready，不能代替 passive Hook 启动 barrier，所有 API 替换测试不证明 Windows 访问成功。


CaptureSession 现有 explicit install/poll/shutdown：安装初始 passive，poll 返回 Waiting/Activated/Continue/Quit；poll 的错误或 Quit 已关闭 Hook 并尝试有界 recovery，调用者另读 shutdown_report 的 unhook/recovery 两个结果。shutdown 幂等；必须显式调用，Drop 只停 Hook、不 SendInput。生产链接测试保留这些入口，已在 Windows x64 Rust 1.99.0 执行通过。16 个新增 mock tests 在 Linux 运行，不证明桌面采样/Hook 安装或发送成功；2s/20ms 仅期限和等待参数，不是墙钟实时保证。neutral barrier 现有组件不补足 repeat/物理 Pass 冲突/紧急退出/监督/可执行 host，真实验收仍等完整接入。


SessionStep 新增 Emergency：固定原 physical Ctrl+Alt+Shift+F12 连续 >=1s，callback/control 都在相同 delivery time 的 actor 工作之前检查。15 个新增离线 tests 覆盖 threshold、取消、重映射、注入、同刻 up/held deadline 与 close-first 恢复；不替代 Windows 实际输入。真机增加按住期间无新事件、左右/AltGr 布局、两个键盘组合、映射紧急键、Hook 移除/线程停滞，以及恢复短返回验收。允许跨键盘组成是合并 Hook 流的明确边界；独立监督和可执行 host 仍需接入。


活动访问保护新增 metadata-only check_input_access 与 AccessLease（100ms refresh/500ms expiry），不新增 Win32 ABI；CaptureSession::poll 生产链接路径包含此检查，Windows x64 本机生产链接已通过；Windows CI 工作流仍未执行。16 个新增 Linux/mock tests 验证零 VK 调用、持键更新不丢 binding、权限/metadata 错误、callback 期间 stop 与迟到租约。真机需增加持键切换普通/管理员前台、安全桌面、拒绝 token/desktop access 与检查耗时；观察实际捕获释放、accepted ups 和 UIPI 失败。该层不能替代 Hook 存活/独立进度监督或 WTS 断连验收，pre/post 与检查间隔竞态必须保留在结果中。


## WSL2 调用宿主 Windows 工具链

本机 Windows `cargo.exe` 位于用户 `.cargo/bin`，现有 stable 为 1.99.0。新增 `tools/validate_windows.ps1`：vswhere 自动定位 MSVC，源码、cache/target 放入唯一 Windows 本地 TEMP，先 SDK /Zs，再 tests/clippy；保留 `source-manifest.json` 与日志。仅复制 registry cache/index 作为可选 seed，不复制 credentials/config，不更改默认工具链。默认选择固定 1.97.1；未安装时明确拒绝，可显式指定已安装 stable。使用 PowerShell，无需 Windows Python。

从 WSL 项目根目录执行：

```bash
win_source=$(wslpath -w "$PWD")
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass \
  -File "$win_source\\tools\\validate_windows.ps1" -Toolchain stable -Arch x64
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass \
  -File "$win_source\\tools\\validate_windows.ps1" -Arch x86 -SdkOnly
```

`-Offline -SeedCargoHome <Windows-cargo-cache-path>` 可复用已有锁定依赖；输出的 `ValidationRoot` 包含本次独占日志/快照。ExecutionPolicy Bypass 只作用于该进程。Codex 受限沙箱内 WSL interop 的 vsock 返回 EPERM 时需按权限流程在沙箱外执行；此次已授权并成功执行，无自动审批拒绝。直接把 Windows Cargo cache/target 放在 `\\wsl.localhost` 曾出现 SQLite locked 和长时间停留，已结束该次独占进程树，再改 Windows-local 目录；不要把缓存锁警告/存活 PID 当作测试成功。

此次发现并修复真实 ABI 问题：SDK MSG 的 lPrivate 仅在 `_MAC` 分支，Windows Rust 删除该字段；x64 size 48 含尾部 padding，x86 size 28。Rust/C oracle 同步修正，x64/x86 实际 SDK 均通过。公开 MSG 文档展示保留字段，具体条件以安装 SDK header 为准，见 ADR-0041。Linux portable mock 的 `/sys/...` 在 Windows 不含 drive prefix，改成 host-absolute mock identity，保留生产检查/全部故障断言；新 Clippy chunks lint 采用 as_chunks，Linux 1.97.1 也通过。

本机测试日志及源 hash 副本位于 `target/validation/windows-20261009-x64-1.99/`（Git ignored），报告包含路径。Windows 432 测试包括 portable Linux mocks 和 103 Windows unit tests、1 个生产链接测试；Linux 专有 ioctl/进程 tests 只在 Linux 跑。未安装实际 Hook、调用 SendInput 或加载 driver，完整输入恢复/性能/桌面验收继续保留。
