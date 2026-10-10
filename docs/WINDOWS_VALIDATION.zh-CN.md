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


2026-10-09 原子Job启动：[报告](validation/windows-2026-10-09-atomic-job.json)记录固定1.97.1 x64/i686各523 tests、188 Windowsunit、native_link、all-targets Clippy/fmt/SDK通过，145源文件hash一致。创建时STARTUPINFOEXW的Job属性取代创建后Assign，SDK核验EX 112/72、attribute offset104/68与新API/constants。两种check_job.exe例程与父死亡fixture真实运行exit0；父由测试自己启动，只产生own-exe child，观察器先保留live child句柄再终止own parent，等待原child退出，不依赖Rust Drop、不按marker PID kill。结果归档ignored target/validation/windows-20261009-atomic-job-{x64,x86}-1.97.1。复现时先在验证快照中build check_job，再执行：

```powershell
powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass `
    -File tools/check_windows_job.ps1 -ValidationRoot '<Windows-local ValidationRoot>' `
    -Target x86_64-pc-windows-msvc
```

目标也可为i686-pc-windows-msvc；脚本所需exe位于该root的target/target-triple/debug/examples，使用同一快照tools脚本。首轮SDK有删除旧接口后的stale void引用，已在编译失败exit2后修正并重跑，不是通过证据。父死亡观测覆盖已resume fixture；创建时Job契约不提供跨桌面输入安全或全局键清理证明。原生IPC、worker自身启动核验、progress supervisor、Hook liveness和完整host仍待接入；见ADR-0049。


2026-10-09 overlapped进度管道：[报告](validation/windows-2026-10-09-progress-pipe.json)记录最终固定1.97.1双架构各534 workspace tests、199 Windowsunit、native_link、all-targets Clippy/fmt/SDK通过，150源文件hash一致。新增SDK核验OVERLAPPED 32/20与event offset24/16、SECURITY_ATTRIBUTES 24/12、pipe/event/cancel/security/RNG原型及flags/constants。运行 `cargo run --locked -p keyweave-windows --example check_progress_pipe --target x86_64-pc-windows-msvc`（i686同理）仅创建本进程私有pipes/events，以共享v1固定24/40-byte协议和显式fixture epoch10→11完成往返，另实测EOF与取消empty pending read。两个宿主目标最终实际exit0，close报告全部complete；无Hook/SendInput/driver。例程的1ms sleep仅测试外层deadline循环，生产Read/Write无sleep/wait。

首轮native通过后发现Connect completion byte count按Microsoft文档未定义；强化旧connect test为73复现InvalidData，再修正忽略count，最终全套/两种原生例程重跑并核对同快照。final日志/manifest/cmd在ignored target/validation/windows-20261009-progress-pipe-{x64,x86}-1.97.1，before回归与Linux最终日志在linux-20261009-progress-pipe。fixture count不能冒充真实CaptureSession进度，same-process往返不代表child句柄交付/身份核验/Job超时监督已完成。取消未确认时必须保留pending资源；Drop fallback不是全回收证明，见ADR-0050。


2026-10-09 管道元数据补验：[报告](validation/windows-2026-10-09-pipe-metadata.json)保存最终固定1.97.1双架构各537 tests、202 Windowsunit、native_link、Clippy/fmt/SDK及两个真实无输入 pipe smoke exit0。新SDK原型 GetNamedPipeInfo/GetNamedPipeHandleStateW 与 FILE_READ_ATTRIBUTES/endpoint/message/NOWAIT constants 已核验。首轮 native 的 InvalidData 发现查询返回包括 PIPE_REJECT_REMOTE_CLIENTS（本机 server0x9/client0x8）；修正严格比较和 mock 后完整重验，初轮编译通过不能替代失败的 native 验收。最终150源文件快照hash一致；Windows11及跨进程启动/监督/真实输入仍待执行，元数据不证明任意handle的async属性。


2026-10-09 精确进度端点交付：[报告](validation/windows-2026-10-09-channel-delivery.json)记录最终固定1.97.1双架构各546 tests（211 Windowsunit）、Clippy/fmt/SDK和152源文件快照一致。SDK新增 HANDLE_LIST/STARTF_USESTDHANDLES/INHERIT/share flags与SetHandleInformation原型。先编译 `cargo build --locked --offline -p keyweave-windows --example check_job --example check_job_progress --example check_progress_pipe --target <target>`，再在对应Windows-local ValidationRoot调用 `tools/check_windows_job.ps1 -ValidationRoot <root> -Target <target> -ProgressDelivery`。脚本只启动本次自己的fixture、强制停止own parent并以retained handle观察child退出；两个架构最终exit0，另重验same-process pipe roundtrip。挂起时端点存活、恢复exit259/EOF与进度通道启动后的父死亡保护已实测，不是worker receipt/真实CaptureSession/Hook活性验收。首轮production CreateFileW重复声明类型不一致被Clippy定位，修正后全套重跑，旧日志单列不作通过证据。worker标准管道是overlapped，后续须经过启动核验并交给private异步端点，禁止直接用同步std::io读写。


Worker 启动与跨进程验证入口（ADR-0052）：在 Windows-local 验证快照中运行下列命令，`<target>` 可取 `x86_64-pc-windows-msvc` 或 `i686-pc-windows-msvc`，使用与 SDK 检查相同的 vcvars 环境和固定工具链：

```powershell
cargo +1.97.1 build --locked --offline -p keyweave-windows --example check_worker_startup --target <target>
& "$env:CARGO_TARGET_DIR/<target>/debug/examples/check_worker_startup.exe"
```

例程只创建自己的 detached Job child 与私有管道，在真实 child main 准入后以 private async copies 读取24字节 challenge、返回40字节 receipt；测试 epoch10→11、冻结拒绝、重复 claim、错误父 PID、直接启动拒绝、同步模式拒绝、实际exit259/EOF与完整父端关闭。它不安装 Hook、不调用 SendInput，也不证明 CaptureSession 实际 poll 活性。Job/端点交付/父死亡旧例程需一并回归。Native 查询签名依据官方 DDI；SDK winternl.h 未声明 NtQueryInformationFile，不将 compile-only oracle 描述为独立验证其原型或 WDK 检查。真实 main 的调用时机由入口负责，token 不提供 OS 初始线程鉴权。

2026-10-09 最终启动准入验收：[报告](validation/windows-2026-10-09-worker-startup.json)记录固定1.97.1 x64/i686各557 tests（222 Windowsunit）、Clippy/fmt/SDK通过与157源文件快照一致。两个架构 `check_worker_startup` 实际exit0；旧Job/父死亡/pipe例程也在最终detached标志下全部通过。首轮 native 发现 CREATE_NO_WINDOW Job名单含两个PID，修正detached后严格只接受当前PID，失败/诊断记录另列，不能把初轮编译通过当原生验收。Win11/真实输入/独立监督/完整host仍待实现与验收。

Windows 独立监督的无输入入口（ADR-0053）：用与 SDK 相同的 vcvars/固定工具链环境，在 Windows-local 验证快照执行：

```powershell
cargo +1.97.1 build --locked --offline -p keyweave-windows --example check_supervisor --target <target>
& "$env:CARGO_TARGET_DIR/<target>/debug/examples/check_supervisor.exe"
```

目标分别取x86_64-pc-windows-msvc/i686-pc-windows-msvc。例程父端是真正的Supervisor，child在真实main完成WorkerStartup；healthy完成两轮fixture进度并exit259，stall已回应后冻结事件循环但保留运行的后台线程，startup_stall不回应，wrong_run返回不匹配身份，stopped_stall已发送Stopped但不退出，requested由父主动请求停止。所有非healthy案例必须由自有Job终止并由process handle实际观察退出；父端随后完整显式close。后台线程在worker开始协议前已至少运行一次，不读写进度。此epoch10→11→12来自fixture，不能当CaptureSession实际poll/Hook健康或全局注入键恢复。

完整Rust测试/Clippy结束后再运行原生例程，避免共用target目录的并发构建；每次unique snapshot/cache/target，最终manifest核对当前源码。存在的日志必须与已确认终止的具体验证进程关联，单独通过native例程不能代替workspace tests/Clippy。

2026-10-09 最终独立监督：[报告](validation/windows-2026-10-09-supervisor.json)记录Linux606、固定Windows1.97.1双架构各569 tests/234 Windowsunit、Clippy/fmt/SDK及160文件hash匹配，验证进程终止exit0。两个Supervisor原生六场景实际通过；x64组合native exit0，x86组合先因旧挂起交付fixture异常0xc000004b退出1，同源码受影响路径重跑exit0并补齐父死亡。失败与重跑日志均保留，异常原因未确定；早先一次x86对象路径构建失败也独立归档，不拿native通过替代失败的编译检查。最终源码的queued fast-reply/延后Write ack回归证明旧逻辑误报Identity，最终全套重新验收。完整host/真实CaptureSession/Hook活性/Win11与实际键盘仍未验收。

2026-10-09 捕获worker接线：[报告](validation/windows-2026-10-09-capture-worker.json)记录Linux616、固定Windows1.97.1双架构各579 tests/244 Windowsunit、Clippy/fmt/SDK、162源文件hash一致与验证进程terminal0。新增10worker tests执行真实Session.poll/handler/ledger代码，OS入口用mocks；native_link只链接Worker<SendInputBackend>生产方法地址。本轮没有运行真实捕获worker/Hook/SendInput，也没有用旧无输入fixture冒充新loop系统验收。原生运行需完整可信exe/main/配置和独立Supervisor host，后续按实际输入授权与实机门槛验证；已有CLI尚不能启动这个loop。


## 真实 worker 入口补验（2026-10-09，ADR-0055）

固定 Rust1.97.1/MSVC19.44.35229.0/SDK10.0.26100.0 在Windows10 build19045执行x64与i686独占local TEMP快照：各588 workspace tests、all-targets Clippy -D warnings、fmt及原SDK compile-only oracle通过，最终验证与fmt进程确认exit0；各167源文件SHA匹配最终工作区。实际debug binary的初始main接入WorkerStartup、配置、SendInputBackend/Worker，生产路径编译链接；未做release/安装验收。[报告与artifact hashes](validation/windows-2026-10-09-worker-entry.json)。

实际binary test检查错误argv静默64，未受保护直接调用静默65；真正Supervisor/owned Job启动后claim成功，再打开不存在配置路径，真实观察Exited code66且epoch0，两个parent进度pipe明确清理。该test不创建配置目录、不安装Hook、不调用SendInput、无driver/键盘操作，仅证明准入→配置失败→进程退出。首轮x64在真实code66时先见receipt EOF，原test要求reason=None而失败terminal101；保留原log/manifest/entry，允许保留独立EOF/BrokenPipe channel reason后双架构全部重验，未更改生产Supervisor或抹去故障原因。code66仍必需，lease/其他reason不予通过。

新增6个portable库tests（Linux另有非Unicode路径test）与3个Windows真实binary tests，Linux最终625项及独立main-thread无设备harness通过。源代码无新FFI/SDK声明，SDK stdout已观察而未单独归档；所有原日志/manifest/cmd、前述失败证据位于ignored target/validation/windows-20261009-worker-entry-*，Linux日志在linux-20261009-worker-entry。普通panic hook静默不证明fatal abort或全局输入恢复；退出0不替代owned process signaled观察。完整用户host/控制/诊断、正常捕获/Hook存活/真实输入恢复与Windows11/实机依旧未验证，ADR-0053以前x86异常原因仍未知，不因此轮入口通过称已修复。


## 协作停止补验（2026-10-09，ADR-0056）

最终固定1.97.1/x64/i686各604 workspace tests（Windowsunit255/shared progress12，新增16行为tests）、all-targets Clippy -D warnings、fmt/既有SDK oracle通过，两个170源文件SHA全匹配工作区。初始两Windows快照已通过但缺最终callback/耗尽tests，仅作preliminary归档；旧session确认terminal0后最终重新验证两arch。Linux641+独立main-thread无设备harness/Clippy/fmt/source60通过。原partial tests只更新有界command Read次数，原冻结frame和baseline8/epoch9保留；公开wrapper unused由owned callback/API回归解决，无lint suppression。[报告](validation/windows-2026-10-09-cooperative-stop.json)。

完整validation terminal后才在相同final snapshots fmt/check/build/run native，父进程terminal0。两个arch的check_cooperative五场景：initial/active/standby真实WorkerStartup后接Stop并确认自然exit0（initial/standby epoch0），stop_stall读Stop不退出和startup_stall不读被owned Job有限停止、确认exit0xe04b5701；父两个pipe cleanup完整。旧check_supervisor六场景各arch回归通过，healthy真正259、其他expected reason/停止/exit均核对。共22个自有进程场景无输入，fixture epoch是合成值，未安装Hook/SendInput/driver，不证明实际CaptureSession停止/全局按键恢复。SDK无声明改动，stdout观察而未单独归档。

ignored target/validation/windows-20261009-cooperative-{x64,x86}-1.97.1保留tests/clippy/fmt/source-manifest/validate/native-build/native-smoke.cmd与两个smoke logs；linux-20261009-cooperative及windows-20261009-cooperative-before-owner-*保留初始Read断言/unused-wrapper失败与preliminary快照。request_sent/Cooperative reason不能代替接收/恢复/退出证据，Stop deadline不续lease；全局输入/正常worker捕获/Windows11/完整host仍待验收，ADR-0053 x86异常仍未解释。


Windows 配置文件资源层（2026-10-09）：固定 Rust1.97.1/MSVC19.44/SDK10.0.26100.0，x64/i686 各618 workspace tests、alltargetsClippy/fmt、更新 SDK prototype/constants oracle 通过；各有3个实际自有临时文件 tests、11个新mock/fault tests。Linux652通过。实际文件验收包括读取、写/删/重命名拒绝、8MiB内容与最后 reader 关闭后实际 NotFound；无 Hook/SendInput/系统驱动操作。两份175源hash匹配，验证父进程与独立fmt均terminal0。初次 OVERLAPPED 声明类型冲突的Clippy失败terminal101另归档，未压lint。

[完整证据与限制](validation/windows-2026-10-09-config-artifact.json)、[ADR-0057](ADR-0057-owned-config-artifact.md)。配置跨进程句柄交付/完整host、有效ACL跨用户审计尚未执行；现有路径不是端到端不可变身份，文件close不独立证明删除。上一轮x86进程异常原因仍未知；本轮未重复旧Job fixture作修复证明。原完整开发范围继续。


Windows 配置对象交付（2026-10-09）：固定1.97.1 x64/i686最终各633 workspace tests（279 Windowsunit），SDK/alltargetsClippy/fmt通过，176源file SHA均匹配；fullvalidation terminal0后运行nativefixtures并确认terminal0。新check_config_handoff各arch3例验证真实guard/继承READ文件对象exact synthetic bytes/重复claim拒绝、bad0拒绝、read后stall被ownedJob终止并实际退出，最后close后实际NotFound；bad0例无数据读取。旧三句柄check_worker_startup全例继续通过。真实keyweave-worker还执行UTF8/JSON两无效image、均实际Config66先于安装；不会打开有效配置，无Hook/SendInput/driver。Linux666与独立无设备harness/Clippy/fmt/source60通过。

[报告与原始证据hash](validation/windows-2026-10-09-config-handoff.json)、[ADR-0058](ADR-0058-owned-config-handoff.md)。保留第一次x64立即删除断言失败、第二x64新增preflight误禁止rollback clear及Linux同断言失败日志；最终改为有界实际NotFound/允许必要清理，并全量复验双arch。native epochs仅fixture read进度，不证明CaptureSession；完整Supervisor config factory/parenthost尚未接入，old9项path模式仅legacy。旧x86异常仍未知，不称已修复；正常输入/Windows11/硬件与所有原范围继续。


Windows 受监督配置启动（2026-10-10）：fixed1.97.1双arch最终各642 workspace tests（287 Windowsunit）、alltargetsClippy/fmt/既有SDK通过，178源hash全匹配；fullvalidation父进程terminal0后才nativebuild，native父进程terminal0。新Supervisor真实worker入口在Config66/epoch0退出前禁止清理，退出后独立pipe/writer报告+实际NotFound。entry各进程创建同parent串行；mutex之前两绿快照保留为preliminary，不当final证据。Linux674/harness/Clippy/fmt/source60通过；OS实际只读核验10.0.19045.0。

check_cooperative现可用--config-transfer：各arch五场景四句柄真实guard/原文件对象/Stop/期限/ownedJob/actual exit/cleanup/NotFound通过；legacy五场景与check_supervisor六场景继续通过。read proof按正常Exit0或已回fixture epoch限定，startup_stall强制退出不证明配置读取完成，epochs都非CaptureSession。没有Hook/SendInput/driver/用户配置；Windows11/硬件/正常输入、完整host/nativepreflight仍未验收。旧x86异常未解释，不因本次通过称修复。[报告](validation/windows-2026-10-10-supervised-config.json)、[ADR-0059](ADR-0059-supervised-config-startup.md)。

在对应已核验的Windows-local snapshot、fixed工具链/target环境运行新增无输入模式：

```text
cargo +1.97.1 build --locked --offline -p keyweave-windows --example check_cooperative --target x86_64-pc-windows-msvc
# PowerShell: CARGO_TARGET_DIR 必须指向该 snapshot 的 Windows-local target 目录。
& (Join-Path $env:CARGO_TARGET_DIR 'x86_64-pc-windows-msvc\debug\examples\check_cooperative.exe') --config-transfer
```

x86对应i686-pc-windows-msvc，并使用x86 vcvars环境；不要与正在运行的MSVC fullvalidation并行native构建。例程只操作自有process/pipe/synthetic临时文件，不安装Hook；不等价于完整用户host。


Windows host冷配置准备（2026-10-10）：同worker无损compiler和SessionActor/Engine gate，保留原JSONC image，纯准备不构造backend或启动input。

最终固定 Rust1.97.1：Linux684 workspace tests及独立main-thread无设备harness、alltargetsClippy/fmt/source60通过；Windows x64/i686各652 tests（Windowsunit290、worker lib13、实际prepared文件1）、alltargetsClippy/fmt/既有SDK通过。两个181源文件SHA全匹配最终工作区，fullvalidation父进程34463和随后fmt父进程67228实际terminal0。初次Mapping未实现PartialEq的编译错误改为逐项语义断言，callback测试两处unwrap引起的Clippy失败改为明确成功断言；初始失败与首Windows快照留存，不屏蔽lint。[验证报告](validation/windows-2026-10-10-prepared-config.json)、[ADR-0060](ADR-0060-host-config-preparation.md)。ignored target/validation/linux-20261010-prepared-config、windows-20261010-prepared-config-{x64,x86}-1.97.1和before-callback-assert-x64保留原始证据。未改变FFI/SDK/协议、不重跑无关旧native fixture作本功能证明。旧x86异常原因仍未知；完整host/真实输入与原目标继续未完成。


Windows host 随机启动身份（2026-10-10）：callback外系统RNG产生非零u64 run与非零u32 marker，零值最多4次整pair重抽，无弱fallback。

最终fixed1.97.1：Linux690 workspace+独立main-thread无设备harness、alltargetsClippy/fmt/source60通过；Windows x64/i686各659 tests（Windowsunit296、新6mock+实际RNG/argv1）、alltargetsClippy/fmt/既有SDK oracle通过，两个182源文件SHA匹配工作区。fullvalidation session48694与随后同snapshot fmt session99265实际terminal0，无本轮失败。ignored target/validation/linux-20261010-run-identity及windows-20261010-run-identity-{x64,x86}-1.97.1保留日志/manifest/命令。[报告](validation/windows-2026-10-10-run-identity.json)、[ADR-0061](ADR-0061-host-run-identity.md)。实际生成证明本机系统API调用与固定argv范围，不能证明绝对唯一/认证/完整host/真实键盘；旧x86异常未解释，不以本轮通过称修复。下一步可信exe定位/用户控制/host启动事务，原目标继续未完成。


Windows用户host实际链路（2026-10-10）：

D08 Windows可运行用户host（2026-10-10）：ADR-0062新增workspace包keyweave-host，check-windows离线与run-windows显式identity，非Windows run=78。默认canonical current_exe同目录worker，显式absolute可信developer image，无PATH/shell；不是签名/ACL/原子image验证。冷File/read/无损compile/native+Engine gate→fresh identity→seal→四句柄Supervisor接通真实worker入口。主控制线程独立于worker capture持有全部native owners，同Instant epoch驱动poll；active无stdio/同步配置IO。独立输入线程一个byte/EOF/error设stop flag，不join阻塞读；首次协作停止不延长旧lease，强制stop最多5秒观察actualExit，不自动重启/resume。Exited后独立pipe/config close，bounded去重diagnostics显式截断。启动错误递归独立Config envelope/Job与Resume owner，先Prepared/actualExit后关闭端点/准备reader/writer，超时不称cleanup成功。已知stop在preRNG/preJob取消，封存artifact明确关闭；startup_cancelled与未观察到exit区分，不伪造child退出，检查后新stop仍走协作路径。

最终fixed1.97.1：Linux696 workspace+独立main-thread无设备harness、alltargetsClippy/fmt/source60通过；Windows x64/i686各667 tests（host4unit/4CLI，其中一个实际取消文件test，原Windowsunit296）、alltargetsClippy/fmt/既有SDK通过。两份189源hash全匹配（含新PowerShell工具），fullvalidation55196和随后native/fmt34600实际terminal0。Linux最后新增变化仅在cfg(windows) host文件，其已测compiled代码未变。初版665 tests/native绿快照仅preliminary；修正工具UNC cwd警告后，再补启动取消并最终双arch全量重验，所有旧session先确认terminal后再构建，无本轮失败掩盖。

各arch真实host→ownedJob→check_host_worker无输入fixture五场景通过：active/standby按stdin停止自然child0/host0（standby epoch0），stop_stall/startup_stall旧lease内强制真实child3763033857/host1，invalidPE真实CreateProcess失败与Prepared回滚host1。所有case明确资源报告并实际观察自有image NotFound；fixture只严格main准入/原reader加载/合成进度，不调用Hook/SendInput；有用配置从未传给真实capture worker。工具tools/check_host.ps1可复验同Windows-local snapshot；epoch不是CaptureSession，resources_complete只指返回pipe/config报告、不独立证明Job handle Drop或全局按键恢复。

[设计](ADR-0062-windows-user-host.md)、[最终证据](validation/windows-2026-10-10-user-host.json)。ignored target/validation/linux-20261010-user-host与windows-20261010-user-host-{x64,x86}-1.97.1留存日志/manifest/cmd/最终case stdout+stderr，before-local-cwd与before-startup-cancel保留preliminary证据。下一步原生console Ctrl+C协作控制、详细child诊断/热更新、可信打包与正常CaptureSession/Windows11/硬件；Linuxbootstrap/consumer-ready、D05/GUI/Rustdriver/companion与所有原目标继续未完成，旧x86异常原因仍未知。


2026-10-10 ADR-0063最终验证：固定Rust1.97.1，Windows x64/i686各673tests、all-targets Clippy -D warnings、fmt和更新console SDK oracle通过；两份193源hash匹配最终源代码。fullvalidation22452及随后native/fmt38147均实际terminal0。每arch私有console event0/1×active/standby四场景自然worker0/Cooperative/资源报告完整，delivery/unregister/release各Ok，自有配置文件实际NotFound；旧5个stdin/停滞/启动失败场景通过。Linux702tests及独立无设备main-thread harness通过。

初次原生18187实际exit1：CreateNoWindow仍附隐藏console，被准入拒绝，未进入body、未跑x86原生；保留日志。修正为DETACHED_PROCESS自启动自有helper，保持拒绝既有console，最终重新双arch全量通过。早期fullvalidation通过快照仅preliminary，不代替最终快照。ConsoleProbe factory失败在body前仅有限Drop回退，不能声称独立完整清理报告。详细快照/manifest/session/case及限制见[JSON报告](validation/windows-2026-10-10-console.json)与[ADR](ADR-0063-console-cooperative-stop.md)。软件生成控制台事件未安装Hook/SendInput，不能证明物理键盘/Terminal/RDP/Windows11或完整移植。旧x86异常仍未知，不用本轮通过称已修复。


D08 worker实际退出诊断（2026-10-10）：ADR-0064将WorkerExit移到共享exit模块，按完整u32解释已知8项契约，ExitDiagnostic只含静态code/message/remediation。host只对实际worker_exit=Some显示分类，并保留原exit/reason/resources/diagnostics；None不推断clean，成功判定不变。未知码保留全值，clean不覆盖独立强制原因/cleanup失败；分类不是image鉴权/具体API因果或恢复证明。完整独立终止报告通道继续待做。

最终fixed1.97.1：Linux705tests及独立main-thread无设备harness、Clippy/fmt/source60通过；Windows x64/i686各678tests（新增3共享exit+2host报告tests）、alltargetsClippy/fmt/既有SDK通过。两个194源hash匹配最终代码，fullvalidation60454实际terminal0，后续native/fmt54556实际terminal0。每arch5个真实host无输入场景实际显示clean或未知分类，保留3763033857与Progress(Expired)，invalidPE仍保留CreateProcess失败；全部自有文件实际NotFound。真实缺配置/无效bytes worker测试确认code66共享分类，无Hook/SendInput。[设计](ADR-0064-worker-exit-diagnostics.md)、[报告](validation/windows-2026-10-10-exit-diagnostics.json)。原始证据在ignored target/validation/*exit-diagnostics*；无本轮失败。旧x86异常未解释，真实键盘/完整终止诊断/热更新/可信打包及所有Linux/D05/GUI/Rustdriver/companion原范围继续未完成。


D08独立终止事实（2026-10-10）：ADR-0065新增TerminalEvidence固定0x4b57高16bit+低10bit非零mask，严格reserved/tag/零mask拒绝。TerminalFacts.native_exit保留fault/progress/缺shutdown/Stopped未确认/unhook/recovery/notification未确认/access/session/untracked全部独立事实，cleanup类别优先但不删fault。真实worker完整report/shutdown路径生成mask，显式drop worker、execute返回后才完成Windows完整native exit；早期scalar与clean0保留，无report/shutdown不造细节。host实际Exited后列全部issue，原reason/resources/diagnostics及success判定不变。不增加pipe/IPC或进度租约。notification是原聚合结果，mask不是API具体错误/ledger/全局恢复证明；Linux不可截低byte复用。

fixed1.97.1最终：Linux708tests+main-thread无设备harness、Clippy/fmt/source60通过；Windows各682tests、alltargetsClippy/fmt/SDK通过，两个195源hash全匹配。fullvalidation72317与随后native/fmt94578实际terminal0，每arch7个无输入host场景通过；native完整1263992835/1263993855已从owned Job观察，分别显示2/10个issue并保留Cooperative/resource结果，自有image实际NotFound。初次const Option编译失败改显式match；初Windows x64tests通过但Clippy drop_non_drop失败，metadata guard无Drop，去掉无效drop后重验；原失败证据保留，不改lint/工具链，初wrapper未启动x86。最后fixture修正只在cfg Windows，Linuxcompiled代码未变。[设计](ADR-0065-independent-terminal-evidence.md)、[报告](validation/windows-2026-10-10-terminal-evidence.json)。ignored target/validation/*terminal-evidence*及terminal-before-guard-fix-x64保留原始日志/manifest/cases。无真实Hook/SendInput，旧x86异常未解释，完整诊断/热更新/可信打包、真实键盘及Linux/D05/GUI/Rustdriver/companion全部原目标继续未完成。


D06设备上下文（2026-10-10）：ADR-0066补齐Linux Runtime owned Probe vendor/product→Engine条件，保存完整DeviceInstanceId/generation，其余未证明属性Unknown。新DeviceInventoryScope明确Complete/Partial，旧context缺字段保留旧Complete契约且输出省略，Partial必须输出；Partial存在查询匹配可True、未匹配/空列表不证明不存在，exists_unless不成True。Runtime只提供captured Partial inventory，初poll传入绝对MonoTime、1h有效期/半小时next_wakeup有界刷新，占1个64操作预算，统一seq/revision增加，不另造progress epoch/IO/neutral。准备与刷新失败仍close capture先于output恢复；hot config旧binding up保留。metadata续期不重验物理连接，read/HUP拔除有观测延迟，全系统inventory/hotplug/应用与输入源context合并仍缺失。

最终fixed1.97.1：Linux717tests+main-thread无设备harness、alltargetsClippy/fmt/source60/既有Linux C ABI通过；Windows x64/i686各691tests、alltargetsClippy/fmt/既有SDK通过，两个195源hash匹配。fullvalidation8124与随后fmt16259实际terminal0。新增6runtime（双设备vendor/product、partial存在/不存在、未知属性unless、绝对时间续期与预算、持键热更新、revision/seq耗尽收尾）+2Engine partial+1CLI JSONscope测试。初Linux4个旧assertion因新增context deadline/seq与溢出优先级失败，保留原日志并分别断言无emergency deadline、保留metadata唤醒及实际close顺序，未删语义检查；输出/宏/取消/紧急退出回归通过。无真实系统设备访问/新native输入例程，不重跑无关host fixture作此功能证明。[设计](ADR-0066-linux-device-context.md)、[报告](validation/windows-2026-10-10-device-context.json)，ignored target/validation/*device-context*留存日志/manifest。旧x86异常原因未知，全部完整目标继续未完成。


D06精确设备移除（2026-10-10）：ADR-0067新增Runtime.remove_device完整实例+generation控制事务，先close目标capture，再丢弃其未交付队列、撤销其Engine owners/timers。closed slot保留tombstone，round-robin跳过；其他键盘共享输出与旧热配置binding保留。unknown/stale/double请求不推进time/seq，控制不造progress epoch；最后设备或最后可达紧急退出接口失去走NoDevices/EmergencyUnavailable整体有限恢复，输出短返回先关闭全部输入再flush，报告不等于queued up接受。只供可信controller在普通poll/due前交付，不是自动检测/鉴权。普通ENODEV/HUP仍整体fault，原生hotplug身份绑定与device add/rearm仍缺失。

fixed1.97.1最终：Linux728tests+独立main-thread无设备harness、alltargetsClippy/fmt/source60通过；Windows x64/i686各702tests、alltargetsClippy/fmt/既有SDK通过，两个195源hash匹配。fullvalidation40966和随后fmt39426实际terminal0。新增11个runtime tests覆盖共享owner、旧binding/no alone/after、held/delayed撤销、未交付队列/稳定slot、stale/double、最后设备/emergency、clock/seq耗尽与短输出恢复。初次1个新test失败为不合法mock receipt，改成accepted=1/report_pending=true且验证empty ledger仍须flush；生产契约未放宽，原日志留存。Linux ABI未变，本轮未重跑。全部mock，无真实全局输入/驱动操作，不重跑无关host fixture作为此功能证明。[设计](ADR-0067-linux-device-removal.md)、[报告](validation/windows-2026-10-10-device-removal.json)，ignored target/validation/*device-removal*保留日志/manifest。旧x86异常仍未知，完整原目标仍未完成。


D06 Linux冷准备（2026-10-10）：ADR-0068新增worker::linux_preparation::PreparedConfig与host check-linux。有界读取保留原JSONC bytes，Linux capabilities generation1无损编译、明确DeviceSpec动作/modifier覆盖、Kernel repeat与Engine gate依次验证；保存已验证spec。共享内部编译helper显式capabilities，旧Windows公共行为与callback guard保留。check-linux默认最大native键表/no-LED，输出offline/readiness unverified，不证明physical passthrough、consumer-ready或bootstrap。真实worker仍须先父保护/channel admission，再准备/实际输出和输入准入。

fixed1.97.1最终：Linux736tests+独立无设备main-thread worker_parent harness、alltargetsClippy/fmt/source60通过；Windows x64/i686各710tests、alltargetsClippy/fmt/既有SDK通过，两份196源hash匹配。fullvalidation18001与fmt25486实际terminal0，无本轮失败。新增6准备tests（原bytes/spec/generation与debug、大小边界、bounded读取/IO/UTF8、key+modifier能力、External repeat、无损拒绝）和2CLI tests（Linux PrintScreen原生输出/离线范围、UTF8/禁用规则拒绝）；原Windows拒绝PrintScreen回归保留。Linux ABI未变，本轮未重跑；无真实全局输入/驱动操作，无关native host fixture未作为本功能证据。[设计](ADR-0068-linux-cold-preparation.md)、[报告](validation/windows-2026-10-10-linux-preparation.json)，ignored target/validation/*linux-preparation*保留原始日志/manifest。真实Linux入口、consumer-ready/hotplug/add以及全部原目标继续，旧x86异常未解释。
