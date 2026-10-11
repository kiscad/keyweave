# ADR-0093：Linux 独立 reload 管道继承与认领

日期：2026-10-11。状态：原生端点交付已实现；活动 worker/host reload 控制尚未连接。

## 独立端点

`reload_channel::channel` 在冷父端创建两条 std::io::pipe，所有四个 FD 都是 NONBLOCK/CLOEXEC；部分构造失败依靠唯一 owner 关闭已有 FD。ParentEndpoints 与 WorkerEndpoints 不提供 Clone/Debug，into_parts 消费 owner 并交付两个 owned File，顺序为 reader、writer。请求和确认分别使用不同 pipe，独立于 progress stdin/stderr 与 terminal stdout；无设备 FD、路径 socket、临时文件或额外外部依赖。

返回 File 使 ADR-0091 Parent/RequestWriter/Receiver/AckWriter 可以直接使用。受信调用方必须保留方向、NONBLOCK 和唯一协议读写者，不复制端点给竞争消费者、不改变共享 file-status flags、不把 reader/writer 混入 Stop/progress。ownership 不能阻止调用方自行 File.try_clone 或修改 OS flags；这些受信进程内前提不是对恶意代码的隔离或端点认证。

## 受控继承

Supervisor 新增 `spawn_reporting_config_reload`，同时要求 reporting、sealed config 和 isolated child group；旧 spawn APIs 保留原行为并清除两个 reload 环境选择器。内部在 parent-death hook 和 sealed-config hook 之后，为两个 worker 端分别 F_DUPFD_CLOEXEC 到最低 3，注册持有唯一 File 的 pre_exec hook。hook 只读取 scalar F_GETFL/F_GETFD、校验方向/NONBLOCK、清除选定 FD 的 CLOEXEC；不分配、不访问环境、不锁、不写、不触碰设备。其他可信 Command hooks 不得关闭/rebind 选定 FD 或改变 group/session。

成功 spawn 后立即 drop Command，父端不保留 worker 端或 hook duplicates；子端的两个额外 descriptor 由环境中的 canonical decimal 选择。Supervisor 持有 parent endpoints，`take_reload_endpoints` 只交付一次，不发送正文/确认、不改变 watchdog。调用方获得端点后必须在 Stop、故障或实际退出时独立关闭/丢弃，并将 reload 时间和监督策略接到原 control owner。

## 实际 main 的单次认领

ReportingStartup 新增 claim_reload；每个 startup 只允许一次尝试，包括失败。先重验当前 main 的 parent-death guard，再读取严格 decimal FD>=3 选择器。只在 fcntl 成功复制后构造 fresh owned File，失败不会接管或关闭未经检查的原 FD；copy 保持 CLOEXEC。随后使用既有 pipe_identity 验证 fstat FIFO、指定只读/只写方向、NONBLOCK 和两个 pipe 的不同 dev/ino。

在交付前拒绝任意一个 reload pipe 与 progress stdin/stderr 或 terminal stdout 的 dev/ino 重叠。验证失败关闭所有新 copies，并永久保留 claimed；原始继承 FD 仍保留到真实进程退出，与既有 config FD 合约一致。复制/metadata 快照不是认证或原子保护；受信 main 不得同时启动线程关闭/rebind这些 FD，禁止后续 exec/fork 携带活动设备或被选定的未关闭端点。

当前真实 keyweave-worker main 尚未调用 claim_reload，现有 host run-linux 仍采用旧 sealed isolated spawn。因此此切片不开放实际输入热更新，也不宣称接通用户 reload 命令。

## 验证

Linux 1.97.1 locked/offline workspace/all-targets 907 项 libtest 测试、Clippy -D warnings、fmt、60 项来源校验、现有 Linux C ABI oracle 通过；新 unsafe 仅使用既有 scalar fcntl ABI，复用现有 fstat pipe 验证，未增加 syscall/prototype/外部依赖。

新增 3 项 native metadata/owned pipe unit tests：四端 CLOEXEC/NONBLOCK/双方向正文与关闭 EOF，错误方向/stdio/不存在 FD/blocking pipe/非 pipe/同 pipe 拒绝且保留 originals，任一 reserved dev/ino 重叠拒绝。/dev/null 只作为已有只读非 pipe 对象进行 metadata 验证，不操作输入。

另有独立 actual-main 自建 no-input harness（不计入 libtest 数）：真实 supervisor→guard→sealed config→isolated group→reload inherit/claim→64KiB+17 正文→Rejected base=1 确认→实际 owned child Exit；重复 claim/take 拒绝。旧 reporting-config 模式清除人为残留 reload selectors，缺失 claim 失败后不得重试。此 fixture 不构造 actor、不生成 Accepted/terminal recovery 报告，progress epoch 保持 0。退出期间 supervisor 可能先看到 channel EOF 并对其唯一 owned child 发 SIGKILL；harness 接受实际正常退出或该 SIGKILL，拒绝 panic/其他退出，不能据此称完成协作恢复。

实际测试等待只轮询原 child/原 transaction，5 秒观察预算不重启任何任务。尚未在 reload 传输期间连接实际 supervisor progress pump/lease，也未运行 evdev/uinput/真实 VT、Windows Hook/SendInput、挂起或 driver；不是硬件验收。Windows 原生前序检查仍因此前自动审批额度错误待补，本切片 Linux-only cfg 不提供 Windows 端点。

## 下一步

将端点附到实际 host/session owner 与 worker entry，接 ADR-0092 单任务准备。每次请求/准备结果/ack 工作前后由原控制循环处理 Stop、访问撤销、parent lease 和独立 clock；输入 actor 与 timers 继续推进，reload 不能产生 heartbeat。成功 commit 后 ack 失败保持实际代号，不 rollback/retry；最后再接 watcher/reload CLI。图形桌面、完整规则、Windows driver 与全部原始移植目标仍未完成。

见 [验证记录](validation/linux-2026-10-11-reload-channel.json)。
