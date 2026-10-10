# KeyWeave

用 Rust 开发、面向 Linux 与 Windows 的键盘映射工具。名称取自“编织按键行为”，是本项目的工作名称，尚未核验商标或包名可用性。

当前已实现 Rust workspace、标准 HID 键名、JSONC 验证、基础映射、多动作序列、轻触独立/松开动作、设备/应用/输入源条件与有效期上下文、typed variables 模式切换、按住与延迟 deadline、显式 lazy 修饰键与 binding halt、确定性离线回放、多来源输出所有权及持键热更新；Linux 已有 owned evdev/uinput、单设备与多设备组抓取准入和连接输入帧—引擎—输出账本的有界运行时执行者。另有 challenge 进度协议、独占子进程监督与 exec 前父线程死亡保护，以及 worker 主线程启动核验和继承管道的 WorkerStartup 接收；已有 CLOCK_MONOTONIC/ppoll 与 run_worker 执行函数，已有 owned output event 节点定位与有期限发现；完整启动事务、可运行 worker 入口与桌面就绪尚未接入，也尚未进行成功设备创建/抓取的真机验收。Windows 已有 Scan 1 键码转换、注入过滤、按键身份绑定、输出账本与恢复，以及 cfg Windows 原生 SendInput 发送代码；还有同步捕获绑定与原生 HookSession 消息循环；Windows x64/i686 已用固定 Rust 1.97.1 编译并运行 mock/生产链接测试，会话 actor 已连接共享引擎与输出账本，并可经 ActorHandler 接到 Hook；已有同 epoch 单调时钟和接 deadline 的原生消息等待代码；已有 callback 外桌面/权限/按键 aggregate 采样；已连接 passive Hook 启动与关闭事务和映射前 Ctrl+Alt+Shift+F12 按住 1 秒的紧急退出；Windows 按合并物理输入检测，允许跨键盘拼接。另有 callback 外活动桌面/权限/WTS连接与锁定检查、启动会话身份比较及短期访问租约；物理重复转发与未归属输入的停止/quarantine保护已接入，外部输入共存、逐键碰撞协调及独立进度/session 监督尚未接齐，不能部署运行。系统后端、完整高级规则、GUI 和驱动仍在开发，进度见 STATUS。原始 `../Karabiner-Elements/` 保持不变。

## 构建与使用

工具链固定为 Rust 1.97.1，执行：

```sh
cargo test --locked --workspace
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/basic.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/sequence.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/tap-alone.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/modifiers.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/output-shortcuts.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/device-context.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/text-context.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/variables.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/held.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/delayed.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/delayed-pending.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/lazy-halt.json --assert
cargo run --locked -p keyweavectl -- doctor
```

目前 `validate` / `import-karabiner` / `replay` 都不访问真实键盘，不执行导入脚本；不支持的功能禁用整条规则并返回诊断。`import-karabiner` 只输出无损副本，发现不支持功能时写报告并拒绝生成可激活副本。`examples/karabiner-basic.json` 已可通过验证，并由 tap-alone fixture 离线验证 Caps Lock→Control、独立轻触 Escape、严格超时与滚轮取消行为。

WSL 内安装的 `stable` 经核验为 1.97.1；离线开发环境用 `RUSTUP_TOOLCHAIN=stable cargo ... --offline` 复用已安装工具链，CI 仍使用固定 1.97.1。尚未运行 Windows CI，不将其配置存在视为已通过。Windows x64/x86 SDK ABI、生产 FFI 链接与真机验证入口见 [WINDOWS_VALIDATION](docs/WINDOWS_VALIDATION.zh-CN.md)。

## 阅读顺序

1. [开发指导文档](docs/DEVELOPMENT_GUIDE.zh-CN.md)：可行性、范围、架构、平台方案、实现步骤、验收标准。
2. [源码分析与证据](docs/SOURCE_AUDIT.zh-CN.md)：本地版本、事件流程、关键源码、上游测试与分析边界。
3. [开发任务清单](docs/IMPLEMENTATION_BACKLOG.zh-CN.md)：带依赖和完成条件的 Coding Agent 工作单。
4. [AGENTS.md](AGENTS.md)：后续开发必须遵守的项目约束。
5. [项目状态](docs/STATUS.md)：区分规划、实现和实际验证。
6. [兼容配置示例](examples/karabiner-basic.json)：已通过离线轻触回放；C2 的其他高级行为仍待实现。
7. [当前兼容范围](docs/COMPATIBILITY.zh-CN.md)：已实现字段、拒绝行为和已知偏差。

结论：可以重写规则引擎并移植主要能力。推荐先完成离线引擎和 Linux evdev/uinput 后端，再完成 Windows 用户态版本；Windows 驱动版本是独立工程与发布门槛。不能把 Windows 用户态版描述为 Karabiner 全功能等价版，也不能承诺所有桌面环境都支持应用条件和输入法切换。

分析日期：2026-10-08（Asia/Shanghai）。源码基线：`ec2fea8940f7254a748c052a890ee9db7113d9b1`，`version` 为 `16.3.11`。


Windows 本机验证（2026-10-09）：已用 WSL interop 调用宿主固定 Rust 1.97.1 + MSVC/SDK；x64/i686 各 432 个 workspace 测试、生产 Win32 符号链接、fmt/clippy 和 12 个离线回放，以及 x64/x86 SDK compile-only oracle 通过。使用 [Windows 验证脚本](tools/validate_windows.ps1) 把源码与产物放在 Windows-local 临时目录，避免 UNC cache 锁；入口和证据见 WINDOWS_VALIDATION。[固定版本验证记录](docs/validation/windows-2026-10-09-pinned.json) 已补齐；真实键盘仍待验证，项目工具链固定值未变。

Windows 另有同线程 WTS 通知窗口，绑定本会话变更停止与输出提交 gate；通知资源诊断例程 `check_notifications` 仅创建/注册/关闭隐藏窗口，不安装键盘 Hook。[设计与限制](docs/ADR-0045-windows-session-notifications.md)。

Windows 的桌面/权限检查或访问租约失效也会阻止恢复提交，独立于WTS通知和首个错误；非空或未知输出账本保留为Unresolved。[恢复边界](docs/ADR-0046-windows-revoked-access-recovery.md)。

共享进度协议现在位于 `keyweave-progress`，Linux 与 Windows 复用固定帧/租约校验；Windows 回应须来自 challenge 之后实际完成的捕获循环 poll。[协议与边界](docs/ADR-0047-shared-runtime-progress.md)。Windows 原生进程监督仍待实现。


Windows 已增加自有 Job 的挂起进程管理与无输入 `check_job` 例程，双架构原生编译/资源测试通过。[状态与限制](docs/ADR-0048-windows-owned-job-process.md)；正式进度监督、原子启动与完整 host 仍待实现。


Windows worker 现于进程创建时准入自有 Job，已通过双架构无输入父死亡例程。[原子启动与验证边界](docs/ADR-0049-windows-atomic-job-startup.md)；原生 IPC、进度监督及完整运行入口继续开发。


Windows 进度传输已有自有 overlapped byte-pipe pair，真实双架构无输入往返/EOF/取消例程通过。[传输契约与限制](docs/ADR-0050-windows-overlapped-progress-pipes.md)；worker 句柄交付、启动核验和独立超时监督仍待接入。

Windows worker 启动准入与 private async 进度端点接收已实现，固定1.97.1双架构原生例程完成真实跨进程challenge/receipt往返与拒绝路径；独立supervisor、完整host与实际键盘仍待开发。[设计](docs/ADR-0052-windows-worker-startup-admission.md)、[验证证据](docs/validation/windows-2026-10-09-worker-startup.json)。

Windows 已有独占自有Job child与异步进度管道的Supervisor，真实无输入例程验证lease超时终止、错误回应、Stopped仍存活与主动停止，均等待进程句柄确认退出。[设计](docs/ADR-0053-windows-progress-supervisor.md)、[验证记录及异常限制](docs/validation/windows-2026-10-09-supervisor.json)。真实捕获worker/完整host尚未接入。

Windows Worker执行循环已将启动token、private进度客户端与CaptureSession接齐；回应来自challenge之后实际完成的poll，终止保留主错误和独立恢复结果。[设计](docs/ADR-0054-windows-capture-worker.md)、[mock/生产链接验证](docs/validation/windows-2026-10-09-capture-worker.json)。执行入口见 ADR-0055；配置控制/host与真实Hook输入验收仍待完成。

## Worker 可执行入口

`keyweave-worker` 已增加 Windows 真实 main：父保护核验先于配置加载和捕获安装，配置读取有界且拒绝有损激活，终止检查独立 shutdown 结果后实际退出。它使用私有二进制协议，用户 parent host、控制/热更新与正常键盘验收仍待接入；Linux 捕获入口暂未接入。[设计与退出契约](docs/ADR-0055-worker-executable-bootstrap.md)。


Windows 已增加显式协作停止：合法私有Stop帧让worker关闭捕获、恢复已知输出并退出；父Supervisor用不延长旧lease的期限等待实际退出，超时仍终止自有Job。[设计](docs/ADR-0056-cooperative-worker-stop.md)、[双架构mock/无输入跨进程验证](docs/validation/windows-2026-10-09-cooperative-stop.json)。正常键盘恢复及完整parent host/用户控制/配置交付尚未验收；Linux默认进度协议保持原行为。


Windows 已增加独占配置文件资源层与唯一只读 reader，双架构实际临时文件测试验证内容、共享访问限制与最后 reader 关闭后删除。[设计](docs/ADR-0057-owned-config-artifact.md)、[验证记录](docs/validation/windows-2026-10-09-config-artifact.json)。跨进程配置句柄移交、完整 parent host 与正常键盘验收仍待接入。


Windows 现有显式四句柄启动：Job 持有 sealed 配置 writer，worker 接收唯一只读文件对象并在真实 main 核验后直接加载。双架构原生例程验证跨进程实际 bytes、拒绝路径与退出后文件删除；各633项测试通过。[设计](docs/ADR-0058-owned-config-handoff.md)、[证据](docs/validation/windows-2026-10-09-config-handoff.json)。完整 Supervisor 配置 factory/用户 parent host 与正常键盘验收仍在开发。


Windows Supervisor 已接入四句柄配置启动，并保留失败时的独立资源 owner；只有确认 worker 实际退出后才清理父侧管道与配置 writer。Linux674、Windows x64/x86各642项测试及双架构无输入配置/协作停止例程通过。[设计](docs/ADR-0059-supervised-config-startup.md)、[证据](docs/validation/windows-2026-10-10-supervised-config.json)。完整用户 host、预编译/原生输出准入与正常键盘验收继续开发。


Windows host 已有冷配置准备层：保留原始 JSONC 字节，复用 worker 的无损编译，并在创建进程前核验原生输出与引擎限制；worker 安装前仍独立重验。[设计](docs/ADR-0060-host-config-preparation.md)。完整用户 host、控制流程和正常键盘验收继续开发。


Windows host 已增加每次启动的系统随机 run ID 和注入标记生成，固定两架构 marker 宽度并限制零值重试。[设计](docs/ADR-0061-host-run-identity.md)。完整 host 启动和正常输入验收继续开发。


已增加 `keyweave-host` 用户入口：`check-windows --config PATH` 离线检查；`run-windows --config PATH --identity strict` 显式启动 Windows 会话，按 Enter 停止。需要同目录已构建的 `keyweave-worker.exe`；开发时可显式指定可信的绝对 `--worker` 路径。配置准备、随机启动身份、原文件对象交付、独立进度监督与退出清理已接通。[设计与运行限制](docs/ADR-0062-windows-user-host.md)。目前仍属开发版，正常键盘、控制台信号、热更新及完整发布验收继续。


Windows host 现已接入控制台 Ctrl+C / Ctrl+Break 协作停止，Enter/EOF 控制继续可用；关闭窗口、注销和关机的恢复边界另见[设计](docs/ADR-0063-console-cooperative-stop.md)。两架构原生验证使用自有私有控制台和无输入协议fixture，真实键盘及发布验收仍继续。

Windows host终止报告现显示稳定worker退出码的分类与处理建议，保留独立监督原因和资源结果；未知码不推测恢复状态。[设计](docs/ADR-0064-worker-exit-diagnostics.md)。

Windows终止报告现保留 worker 同时出现的独立故障事实（例如恢复未确认、访问丢失、会话变化），同时保留父侧监督与资源结果。[设计](docs/ADR-0065-independent-terminal-evidence.md)。

Linux runtime已将捕获设备的vendor/product接入设备条件，并区分部分与完整设备列表，避免把未捕获设备误判为不存在。[设计](docs/ADR-0066-linux-device-context.md)。真实设备和完整Linux启动验收继续。

Linux runtime已支持可信控制器按完整device generation移除单个捕获实例，保留其他键盘的按键所有权；自动热插拔监控与重新准入继续开发。[设计](docs/ADR-0067-linux-device-removal.md)。

新增 `keyweave-host check-linux --config PATH`：离线检查 Linux 配置、原生输出与 Engine 可执行性，保留有损配置拒绝；不访问输入设备。真实 Linux 会话启动仍待接通。[设计](docs/ADR-0068-linux-cold-preparation.md)。

Linux 新增显式协作停止组件：受控 worker 先关闭捕获再有限恢复，父 supervisor 在不延长旧租约的期限内等待实际退出；默认旧 worker 协议保持。[设计](docs/ADR-0069-linux-cooperative-stop.md)。完整 Linux 可执行启动与真实输入验收继续。

Linux 已保留销毁输出前的独立终止事实，区分已接受的恢复、REPORT 未完成、未知接受量与预算耗尽；受控 worker 报告同时保留原错误。[设计](docs/ADR-0070-linux-termination-evidence.md)。真实启动与跨进程报告交付继续开发。

Linux 已增加显式第三条终止报告管道，固定帧独立于进度租约；父进程实际确认退出后，以完整帧及 EOF 校验交付。[设计](docs/ADR-0071-linux-terminal-channel.md)。真实主线程无输入例程已接入，完整捕获入口与用户 host 继续。
