# ADR-0235：Windows host 公开 Status/Stop 与 CLI

将已有 native Server/Client transport 接入原 Windows host Supervisor，新增显式 --control-pipe、status-windows/stop-windows。endpoint 采用固定本地 pipe 名称、PID、完整 creation FILETIME 的有界文本描述符，host 冷阶段监听并发布，禁止活跃监督循环内 stdio/文件发现/配置读取；metadata 是实例 fence，不是秘密或 image 认证。public session nonce 独立生成，不复用 worker private run/tag。

可测试的公共状态层校验 Snapshot、维持不可变 public session，并处理 Status/相同 nonce 的幂等 Stop/陈旧 session。Stop 只排队，原 host 下一轮以原 request_shutdown 执行一次，不伪造 signal/progress/epoch/generation/lease。每轮先原 Supervisor.poll/到期/actual exit，再至多一个 public transaction step。Starting 保持 generation None/epoch0；只有 epoch>0 才显示已准备 generation；Stopping 为 StopRequested；Exited 只来自实际退出。recovery_confirmed 本切片始终 false，不能用 code0 推断。

只有实际退出后提供最多 500ms 最终快照服务；unknown exit 不等待也不制造 Exited。公开 IPC 错误保留并请求原 owner 强制 stop，仍观察同 child；独立关闭 snapshot/错误参与最终 host 成功判断。原无控制 run API 保持行为，CLI 显式选择时才开端点。

客户端 CLI 以已有 Client machine 的绝对 1s 事务期限驱动，一 poll/短 sleep，不自动启动 worker；异常后额外关闭最多100ms且不越过事务总体1.1s上限。将已发请求后的不确定结果、StopAccepted 与 actual exit 分开。非 Windows 命令只返回 unsupported，不接触输入。

Windows native host wiring 与新 SDK/link 尚未原生编译执行。Linux 测试覆盖公共状态、endpoint/parser/CLI 准入和既有生产 transport 的 API mock，不能代替真实 host→pipe→Supervisor→worker Windows 验收。GUI 接入、metadata 发现、真机/对抗/包装及原完整范围继续；既有 Windows 原生审批额度拒绝未重试。

验证：Linux 默认新增 5 项（共享状态/endpoint 3、CLI parser 1、实际非 Windows CLI unsupported 1），全量 1993 通过，8 native 默认 ignored；格式/Clippy/源码62/Linux ABI/X11 SDK/regex oracle通过。首次新 test module unwrap lint 已修。新增 Windows public_snapshot_tests 只写入未运行；host Windows 专属模块的 type checking/link/runtime 也不由 Linux 成功覆盖。见 [验证记录](validation/linux-2026-10-16-windows-host-control.json)。

原生后续 gate：在 x64/x86 Windows Rust/SDK 上编译全部 workspace/native_link 与 host tests，运行 public_snapshot_tests；再添加无输入 fixture 验证实际 host cold endpoint→Status Starting→带 nonce Stop→原 Supervisor/同 owned child actual exit→最终 Exited，outstanding private challenge 不造 progress，坏 endpoint/stale session/部分消息/取消和复合清理错误均独立检验。不得用真实 capture worker 代替无输入 fixture。GUI Windows transport/自动发现与完整原目标继续。
