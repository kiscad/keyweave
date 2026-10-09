# Windows 原生验证入口

已通过 WSL2 调用固定 Windows Rust 1.97.1：x64/i686 各 432 个 workspace 测试、生产 Win32 符号链接、fmt/clippy 和 12 个离线回放通过；SDK x64/x86 oracle 均通过。结果和日志 hash 见 [固定版本验证记录](validation/windows-2026-10-09-pinned.json)。此前的 [1.99 x64 验证记录](validation/windows-2026-10-09.json) 保留历史结果。实际输入验收仍待执行，生产代码与 unit-test 的 Win32 API 替换分开验证。

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

`.github/workflows/rust.yml` 已配置两种 MSVC 目标。工作流存在不等于已经执行或通过；真实结果须记录 runner、SDK/MSVC/Rust 版本、target 与 job URL。此前仅使用 Linux rust-std，隔离到 `/tmp` 的 Windows target 下载因 DNS 失败。现已发现并实际使用宿主 Windows 的 Rust/MSVC/SDK：Windows 10 build 19045、固定 Rust 1.97.1（此前也验证1.99.0）、MSVC 19.44.35229.0 / tools 14.44.35207、SDK 10.0.26100.0；本机通过不代表 CI 已运行。

## 真机门槛仍未完成

编译/链接/SDK 检查均不能证明输入拦截或应用接收。待完整可执行会话、neutral/碰撞/重复/紧急退出/监督接入后，按 D08 记录 Windows 11 x64、普通/管理员程序、中文布局、特殊键、Injected、睡眠/安全桌面/RDP，以及部分发送后的实际恢复。不能用上述无需安装的测试替代这组验收。

来源：[Microsoft WinUser.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/WinUser.h)、[INPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-input)、[KBDLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct)、[MSG](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msg)、[MsgWaitForMultipleObjectsEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)。

只读采样新增 sample_input_state 的 WinSta0/UOI_IO/desktop access、foreground token integrity 与无 thread impersonation 检查；此路径依赖 user32/kernel32/advapi32，布局和 symbol retention 已加入验证入口。IdleObserved 不是 neutral/capture-ready，不能代替 passive Hook 启动 barrier，所有 API 替换测试不证明 Windows 访问成功。


CaptureSession 现有 explicit install/poll/shutdown：安装初始 passive，poll 返回 Waiting/Activated/Continue/Quit；poll 的错误或 Quit 已关闭 Hook 并尝试有界 recovery，调用者另读 shutdown_report 的 unhook/recovery 两个结果。shutdown 幂等；必须显式调用，Drop 只停 Hook、不 SendInput。生产链接测试保留这些入口，已在 Windows 固定 Rust 1.97.1 的 x64/i686 执行通过。16 个新增 mock tests 在 Linux 运行，不证明桌面采样/Hook 安装或发送成功；2s/20ms 仅期限和等待参数，不是墙钟实时保证。neutral barrier 现有组件不补足 repeat/物理 Pass 冲突/紧急退出/监督/可执行 host，真实验收仍等完整接入。


SessionStep 新增 Emergency：固定原 physical Ctrl+Alt+Shift+F12 连续 >=1s，callback/control 都在相同 delivery time 的 actor 工作之前检查。15 个新增离线 tests 覆盖 threshold、取消、重映射、注入、同刻 up/held deadline 与 close-first 恢复；不替代 Windows 实际输入。真机增加按住期间无新事件、左右/AltGr 布局、两个键盘组合、映射紧急键、Hook 移除/线程停滞，以及恢复短返回验收。允许跨键盘组成是合并 Hook 流的明确边界；独立监督和可执行 host 仍需接入。


活动访问保护新增 metadata-only check_input_access 与 AccessLease（100ms refresh/500ms expiry），不新增 Win32 ABI；CaptureSession::poll 生产链接路径包含此检查，Windows x64/i686 本机生产链接已通过；Windows CI 工作流仍未执行。16 个新增 Linux/mock tests 验证零 VK 调用、持键更新不丢 binding、权限/metadata 错误、callback 期间 stop 与迟到租约。真机需增加持键切换普通/管理员前台、安全桌面、拒绝 token/desktop access 与检查耗时；观察实际捕获释放、accepted ups 和 UIPI 失败。该层不能替代 Hook 存活/独立进度监督或 WTS 断连验收，pre/post 与检查间隔竞态必须保留在结果中。


## WSL2 调用宿主 Windows 工具链

用户提供的 Windows Rust 入口是 `C:\Users\sam chen\.cargo\bin`，WSL 路径为 `/mnt/c/Users/sam chen/.cargo/bin`。2026-10-09 补充只读核验：直接调用该目录的 `rustc.exe -vV` 成功，当前默认版本为 1.99.0、host 为 `x86_64-pc-windows-msvc`。路径含空格，必须完整引用；此结果仅证明 Windows 编译器可执行，不是本次源码已编译通过的证据。项目仍使用固定 1.97.1，完整构建结果按下文各次报告的源码快照判断。

```bash
"/mnt/c/Users/sam chen/.cargo/bin/rustc.exe" -vV
"/mnt/c/Users/sam chen/.cargo/bin/cargo.exe" -V
```

本机 Windows `cargo.exe` 位于用户 `.cargo/bin`，现有 stable 为 1.99.0。新增 `tools/validate_windows.ps1`：vswhere 自动定位 MSVC，源码、cache/target 放入唯一 Windows 本地 TEMP，先 SDK /Zs，再 tests/clippy；保留 `source-manifest.json` 与日志。仅复制 registry cache/index 作为可选 seed，不复制 credentials/config，不更改默认工具链。默认选择固定 1.97.1；未安装时明确拒绝，可显式指定已安装 stable。使用 PowerShell，无需 Windows Python。

从 WSL 项目根目录执行：

```bash
win_source=$(wslpath -w "$PWD")
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass \
  -File "$win_source\\tools\\validate_windows.ps1" -Toolchain 1.97.1 -Arch x64
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass \
  -File "$win_source\\tools\\validate_windows.ps1" -Toolchain 1.97.1 -Arch x86
```

`-Offline -SeedCargoHome <Windows-cargo-cache-path>` 可复用已有锁定依赖；输出的 `ValidationRoot` 包含本次独占日志/快照。ExecutionPolicy Bypass 只作用于该进程。Codex 受限沙箱内 WSL interop 的 vsock 返回 EPERM 时需按权限流程在沙箱外执行；此次已授权并成功执行，无自动审批拒绝。直接把 Windows Cargo cache/target 放在 `\\wsl.localhost` 曾出现 SQLite locked 和长时间停留，已结束该次独占进程树，再改 Windows-local 目录；不要把缓存锁警告/存活 PID 当作测试成功。

此次发现并修复真实 ABI 问题：SDK MSG 的 lPrivate 仅在 `_MAC` 分支，Windows Rust 删除该字段；x64 size 48 含尾部 padding，x86 size 28。Rust/C oracle 同步修正，x64/x86 实际 SDK 均通过。公开 MSG 文档展示保留字段，具体条件以安装 SDK header 为准，见 ADR-0041。Linux portable mock 的 `/sys/...` 在 Windows 不含 drive prefix，改成 host-absolute mock identity，保留生产检查/全部故障断言；新 Clippy chunks lint 采用 as_chunks，Linux 1.97.1 也通过。

固定版本两次测试日志及源 hash 副本位于 `target/validation/windows-20261009-{x64,x86}-1.97.1/`（Git ignored），报告包含各文件SHA256及路径。每份源码快照全部127个文件hash与提交e0796d2一致（包括fixture与工具）；此前1.99记录只检查94个编译源文件。每个目标的 Windows 432 测试包括 portable Linux mocks 和 103 Windows unit tests、1 个生产链接测试；Linux 专有 ioctl/进程 tests 只在 Linux 跑。未安装实际 Hook、调用 SendInput 或加载 driver，完整输入恢复/性能/桌面验收继续保留。


## 独立固定工具链与回放检查

本次使用独占临时 `RUSTUP_HOME` 安装1.97.1，保留现有宿主 stable1.99.0 和用户默认配置；路径只在当前 PowerShell 及其子进程环境生效。需要复现时，可在 Windows PowerShell 设置新目录，然后调用上面的验证脚本：

```powershell
$env:RUSTUP_HOME = Join-Path $env:TEMP ('keyweave-rustup-' + [Guid]::NewGuid().ToString('N'))
$rustBin = Join-Path $env:USERPROFILE '.cargo\bin'
& (Join-Path $rustBin 'rustup.exe') toolchain install 1.97.1 --no-self-update `
    --profile minimal --component rustfmt,clippy `
    --target x86_64-pc-windows-msvc,i686-pc-windows-msvc
if ($LASTEXITCODE -ne 0) { throw 'Toolchain installation failed' }
& (Join-Path $rustBin 'rustc.exe') +1.97.1 -vV
& (Join-Path $rustBin 'rustup.exe') target list --installed --toolchain 1.97.1
```

初次安装另设临时 `CARGO_HOME` 后，组件全部安装，但 launcher 自更新提示 rustup 未安装于该目录并返回失败。后续改为仅设 `RUSTUP_HOME`、实际查询两个 std targets，再完成真实构建，证明工具链完整；不能仅根据“6 components installed”宣称通过。`--no-self-update` 跳过不相关的 launcher 更新，不忽略组件安装错误。不把该次临时目录设为持久用户变量；重开 PowerShell 恢复用户环境。

两个架构的测试在同一 Windows10 x64 主机执行，i686 可执行文件通过 WoW64 运行。本次两个目标各432 tests且全部0 failed，无改动生产代码。fmt 和回放是对上述已核对hash的同一快照补跑：`cargo +1.97.1 fmt --all --check`，以及对应 `target/<target>/debug/keyweavectl.exe replay --fixture tests/fixtures/<name>.json --assert`。12个fixture为 basic、sequence、tap-alone、modifiers、output-shortcuts、device-context、text-context、variables、held、delayed、delayed-pending、lazy-halt；退出均0。验证脚本本身仍负责 SDK/tests/clippy，fmt/replay须单独补跑并保存日志，不将当前本机完整检查误解为脚本自动覆盖。

SDK /Zs 成功与 MSVC 版本输出已在此次命令结果中观察；未单独存SDK stdout日志。报告准确区分已保存的 tests/clippy/fmt/replay/source-manifest/validate.cmd 与未归档的 SDK 输出。报告固定源码提交，后续文档提交不等于重新构建过修改后的源码；代码变化后重新生成快照/证据。GitHub Actions 与真实输入验收仍未执行，不能由这组本机编译检查推断通过。


Windows 物理重复转发现已接入 SessionActor（[ADR-0042](ADR-0042-windows-physical-repeat.md)）：mock 验证按原来源保留末端、旧配置/held、更换当前修饰、repeat:false、注入 bypass 与失败恢复。真实验收需记录 passthrough/重映射重复速率、宏前缀仅执行一次、held 到期后的目标变化、修饰键切换、源释放停止重复，以及同scan多个键盘的合并流限制。当前策略只转发实际物理Repeat，不自行设定重复延迟/速率，不保证源未产生Repeat时的软件重复。

本次 Repeat 实现已在固定1.97.1 x64/i686 各通过445 workspace tests、生产链接与clippy，SDK oracle也通过，见 [Repeat验证记录](validation/windows-2026-10-09-repeat.json)。旧432测试记录对应功能接入前的源码快照，保留为历史证据。


未归属输入保护见 [ADR-0043](ADR-0043-windows-untracked-input.md)。真机需区分OwnInjected与外部自动化/触屏Unicode/unsupported键、外部注入交付时机、发送中fault后下一批拒绝、故障先发生再有旁路、空账本停止与submitted/unknown的Unresolved；保留原主因与ShutdownReport.untracked_input/unhook/recovery。新策略在active旁路时停止，尚未提供逐键共存；callback触发emergency若同时放行down/repeat可能Unresolved，无旁路的idle emergency仍有限恢复。不能把这种保守停止视为完成所有碰撞/紧急恢复真机验收。

未归属保护实测：固定1.97.1 x64/i686各460 tests、生产链接、clippy与SDK oracle通过；[验证记录](validation/windows-2026-10-09-untracked.json) 固定本次源码manifest/hash。前文432/445计数对应历史功能切片，保留各自记录。


WTS查询新增 wtsapi32 链接与ProcessIdToSessionId，SDK oracle核验Unicode WTSINFOEXW/level1大小、offset/alignment、enum/flag及WINAPI prototypes；Rust/SDK必须在两个目标共同通过。只接受本地本进程session的Active/Unlock与startup identity，API buffer用WTSFreeMemory，不读取账号字符串或padding。检查在callback外，metadata-only正常循环零VK扫描。详见 [ADR-0044](ADR-0044-windows-wts-access.md)。

只读诊断：`cargo run --locked -p keyweave-windows --example check_access --target x86_64-pc-windows-msvc`（i686也可）。或运行Windows-local验证目录的 `target/<target>/debug/examples/check_access.exe`。只输出input_access observed/unavailable与错误类别，不记录账号、session时间、foreground内容或按键；不安装Hook/注入，成功不是neutral/capture-ready。实际锁定/RDP断连/注销/快速重连与通知丢失仍需完整host和测试系统验收。


2026-10-09 WTS补验：[验证报告](validation/windows-2026-10-09-wts.json)记录固定1.97.1 x64/i686各477 tests、143 Windowsunit、native_link、all-targets Clippy及双架构SDK oracle通过，132份源码hash一致。宿主x64例程真实执行一次，返回input_access=observed/exit0；只读metadata成功，不安装Hook、不调用SendInput、不采样VK。完整记录在ignored target/validation/windows-20261009-wts-{x64,x86}-1.97.1，x64另含metadata-probe.log。未实测锁屏/RDP/注销状态变化，不能据此声明这些转移或实际键盘安全恢复已验收。

`cargo run --locked --offline -p keyweave-windows --example check_notifications` 是明确调用的通知资源smoke：查询本进程session，创建不显示/不激活的owned窗口，注册THIS_SESSION后注销、销毁、注销class，成功打印session_notifications=registered_and_closed。它不泵消息等待变更，不安装Hook/调用SendInput/键采样；不能替代锁屏/RDP/注销实测。新增WNDCLASSW布局、消息/样式/通知枚举与API原型须通过双架构SDK oracle。

2026-10-09 通知窗口实测：[报告](validation/windows-2026-10-09-notifications.json)记录最终1.97.1双架构各496 tests、162 Windowsunit/native_link/Clippy/SDK通过，136源码hash匹配；两种check_notifications.exe真实注册/关闭资源各一次exit0。原始probe log为Windows PowerShell Tee-Object生成的UTF16，连同完整编译日志/manifest/cmd在ignored target/validation/windows-20261009-notifications-{x64,x86}-1.97.1。没有真实状态变化、键盘操作或驱动验收。

ADR-0046把metadata/lease失效（无WTS消息）也接入recovery gate；8新增mock测试及5旧测试的保留ledger/禁恢复证据不能代替真实UIPI/锁屏/断连。该改动无新增native API/ABI，仍通过双架构编译/link/tests验证实际cfg Windows路径。先前notification资源smoke保持历史证据，本改动不重复真实窗口/键盘操作。

2026-10-09 access-loss补验：[报告](validation/windows-2026-10-09-access-loss.json)记录最终1.97.1 x64/i686各504 tests、170 Windowsunit/native_link/Clippy与既有SDK oracle通过，两份136源码hash一致。完整证据在ignored target/validation/windows-20261009-access-loss-{x64,x86}-1.97.1；Linux另存单项修正前失败和最终全套记录。本次没有真实窗口/键盘操作，不把mock访问撤销当作真实桌面/锁屏/UIPI恢复验收。

共享keyweave-progress与Windows adapter的validation继续用全部workspace与production native_link；client泛型receive/publish地址只证明cfg Windows链接，不执行管道IO或Hook。mock测试使用真实CaptureSession协调器及可控stream，但不证明native transport/Job Object/进程监督。阶段Stopped不代表输出清理或child exited；监督验收必须另取得实际Child exit status。

2026-10-09 共享进度实测：[报告](validation/windows-2026-10-09-shared-progress.json)记录固定1.97.1 x64/i686各512 tests、177 Windowsunit、7 shared-progress tests、native_link/Clippy/既有SDK通过。两个139源码hash与最终代码匹配，完整证据在ignored target/validation/windows-20261009-shared-progress-{x64,x86}-1.97.1。Linux549全套及真实原有子进程测试通过，不等于Windows native supervisor已实现；本次没有真实输入操作。


2026-10-09 owned Job补验：[报告](validation/windows-2026-10-09-job-process.json)记录固定1.97.1双架构各521 workspace tests、186 Windowsunit、production native_link、all-targets Clippy/fmt与扩展SDK oracle通过，143个源码文件hash一致。SDK新增Job limit/STARTUPINFOW/PROCESS_INFORMATION布局、flags/class/wait constants与WINAPI prototypes。无输入例程 `cargo run --locked -p keyweave-windows --example check_job --target x86_64-pc-windows-msvc`（i686同理）只启动当前exe的固定fixture：先观察挂起，再resume验证含空格/引号/反斜杠/中文的argv与signaled exit259；另一child只park，由own Job停止并观察固定退出码。两种宿主真实运行exit0，无Hook/SendInput/driver。原始UTF8日志/manifest/cmd见ignored target/validation/windows-20261009-job-process-{x64,x86}-1.97.1。此例程证明显式停止与实际退出；没有模拟父崩溃或验证kill-on-close父死亡，不覆盖Create→Assign崩溃窗口。完整启动事务/IPC/进度监督仍待完成，见ADR-0048。
