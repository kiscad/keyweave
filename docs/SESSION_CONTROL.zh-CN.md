# Linux 本地会话控制

当前提供 Status 与 Stop，沿同一个受监督 host/worker 生命周期工作。GUI、CLI 均使用同一客户端；没有新建抓取入口。此功能不启动会话、不应用编辑器配置，不提供 Resume、设备列表或 Event Viewer。Windows 公开 named pipe 控制仍待接入。

## 启用与使用

先选择当前用户拥有、权限为 0700 的现有目录，例如 `$XDG_RUNTIME_DIR/keyweave`。在已经通过相应桌面/设备准入验收的 `run-linux` 命令中增加：

```text
--control-socket /absolute/private-dir/control.sock
```

默认不创建控制端点。绑定在 worker 启动之前完成，失败不会启动 worker；已有路径一律拒绝，不覆盖或自动删除。控制目录和 socket 必须与调用者同用户；root host 不接受普通用户 GUI。控制功能不使 WSL2 的普通终端获得 VT/X11/Wayland 捕获能力，原有准入要求保持。

CLI 查询不打开输入设备或启动 worker：

```sh
keyweave-host status-linux --socket /absolute/private-dir/control.sock
keyweave-host stop-linux --socket /absolute/private-dir/control.sock --session 123
```

把 `123` 换成查询返回的 `public_session`。它是独立生成的公开会话标识，用于拒绝旧窗口的 Stop，不是 private worker/watchdog/reload run ID，也不是授权密钥。旧标识收到 StaleSession，CLI 返回 1；StopAccepted 返回 0 仅代表请求交付成功，不代表已经停止或恢复。

GUI 展开 **Local session status and stop**，输入 socket 路径并 **Query host**。之后每 200ms 请求状态，显示最后观察的 endpoint、时间、状态、实际进度计数、已观察的 generation、退出及 worker 恢复事实。**Request stop → Confirm stop request** 绑定当时的 endpoint/public session，超过 2 秒的观察或 endpoint/session 变化不能继续确认。**Disconnect observer** 停止后续自动查询；已经发出的请求可能完成，不是 Stop 的撤回。关闭 Settings 不终止独立 host。

## 状态含义

| 状态 | 能确认的事实 |
| --- | --- |
| Starting | 尚无已确认 Active 进度；不证明输入已经抓取 |
| Active | host 最近观察到了实际 Active progress；不是设备/消费者 readiness 证明 |
| StopRequested | 已排队或正在停止；不证明进程退出、ungrab 或恢复 |
| Exited | 同一个 owned Supervisor 实际观察到进程退出；退出码与恢复事实独立 |
| Unobserved | 退出观察预算耗尽，不能声称进程已退出 |

Code(0) 不会自动令 recovery_confirmed 为 true；该位来自独立 worker 终止报告，只说明其自有资源/账本事实，不证明全局键盘状态。host fault 也是观察时的事实，不预言随后 auxiliary/file cleanup 必定成功。socket 消失、EOF、超时或 GUI 断开都不能代替 actual exit。

host 先处理每轮监督、原 deadline 和 actual exit，再处理一个有界 IPC batch。公开请求不增加 progress epoch、不续 worker lease。Stop 交下一轮同 owned pump，重复请求不延长协作停止 deadline，不伪造 SIGINT。只有确认进程退出后才提供最多 500ms 的最终状态查询窗口；错过该窗口的客户端保持“退出未确认”，也可查看原 host 的终止报告。

## 协议与所有权

Linux pathname Unix stream socket，现有 owner-only 目录、socket 0600，客户端与服务端均使用 SO_PEERCRED 检查同 euid。连接时凭据是认证边界，不承诺处理任意 credential-changing 进程；目前是同用户控制，尚不是跨用户或跨桌面会话 broker 授权。目录路径依赖可信用户目录前提，不是完整 ancestor/管理员竞态证明。

每连接一条请求和一条回复，固定帧后必须 write EOF。部分帧保留进度，额外字节/坏 reserved/version/id 拒绝。至多 4 个 client，单 client 固定 1 秒，慢字节不续期；每次 server poll 至多 1 accept，每 client 至多 1 read/1 write。client 使用 NONBLOCK/CLOEXEC，总传输期限最多 5 秒，GUI/CLI 使用 2 秒。冷路径验证与 OS 调度不构成硬实时保证。

server 只删除其绑定时记录的 dev/inode 对应 socket；路径被替换时报独立清理错误，保留新对象。没有 stale socket 扫描或自动清除。监听器故障进入原 host 的 fail/stop/observe 路径，恶意/坏 client 只关闭自身连接。

### KWCC/v1 请求：32 bytes

| offset | 内容 |
| --- | --- |
| 0..3 | KWCC |
| 4 | version=1 |
| 5 | Status=1 / Stop=2 |
| 6..7 | reserved=0 |
| 8..15 | 正 u64 request ID，little endian |
| 16..23 | Status 为 0；Stop 为 expected public session ID |
| 24..31 | reserved=0 |

### KWSC/v1 回复：64 bytes

| offset | 内容 |
| --- | --- |
| 0..3 | KWSC |
| 4 | version=1 |
| 5 | Status=1 / StopAccepted=2 / StaleSession=3 |
| 6 | Starting=1 / Active=2 / StopRequested=3 / Exited=4 / Unobserved=5 |
| 7 | no exit=0 / exit code=1 / signal=2 |
| 8..15 | 原 request ID |
| 16..23 | 当前 public session ID |
| 24..31 | 实际观察的 progress epoch |
| 32..39 | 最近观察的 generation；0 表示未知 |
| 40..43 | u32 exit value；无 exit 必须为 0 |
| 44 | worker recovery_confirmed，0/1 |
| 45 | host fault observed，0/1 |
| 46..63 | reserved=0 |

只有 Exited 能带 exit；只有 Exited 能声明 recovery；Active 必须有正 epoch/generation，Starting 不得假造它们。只包含状态数值，不包含按键、设备标识、配置、路径或窗口标题。未来 Apply/reload/Event Viewer 不能滥用 reserved 字段或混用私有 progress 帧；需另行版本/权限/有界 payload 设计。

## 验证

```sh
RUSTUP_TOOLCHAIN=stable cargo test --workspace --locked --offline
python3 tools/verify_session_control_native.py
```

普通回归 1938 项通过，其中 8 项真实 socket 测试默认 ignored；当前沙箱创建 AF_UNIX 实际返回 EPERM，因此原生 gate 在已获准的 IPC 可用环境中单独运行，8 项均通过，不能把 ignored 记为通过。

native gate 包含 3 个真实 transport/path/partial/timeout cases、4 个实际 CLI/host cases、1 个 GUI client case。完整 host case 显式构建 session-control-fixture：验证 startup guard 和密封配置，读取但不回复 challenge，接受合作 Stop，发送 synthetic no-input 终止报告；actual host→socket→原 Supervisor→owned child→actual exit→最终 Status 全链 epoch 始终为 0。它从不构造 Runtime 或设备，不证明真实输入恢复。fixture 不作为发行 worker。

既有 C oracle 增加 sockaddr_un/ucred、SO_PEERCRED/flags/函数签名检查。实际跨 UID 对抗进程、Windows pipe、桌面 GUI 显示与真实捕获仍未验收。见 [ADR-0229](ADR-0229-local-session-status-stop.md) 与[验证记录](validation/linux-2026-10-15-local-control.json)。

## Windows 控制开发状态

ADR-0230 已提供冷阶段 `ControlIdentity` 与明确用户/SYSTEM 的 `ControlSecurity` 权限资源；公开 Windows pipe、Status/Stop CLI 与 GUI transport 尚未实现。主体比较包含用户 SID、登录 LUID 与 session；任意 PID 比较结果不构成 pipe-peer 认证。新增 Windows SDK 和生产链接检查尚未原生执行。后续 transaction、取消与 peer 绑定步骤见 [ADR-0230](ADR-0230-windows-control-principal.md)。

ADR-0231 已实现独立的 Windows 服务端/客户端消息事务（`keyweave_windows::session_control`），沿用 KWCC/KWSC。一连接一请求，完整 Read 后鉴权，再执行一次业务处理器；回复后客户端主动关闭，服务端有界观察关闭并清理后复用。默认 1 秒绝对事务期限和额外 100ms 关闭期限，每 poll 至多一个 transport 方法或一次 handler；无内部重试/阻塞 flush。机器只借用 transport，不拥有 OS 资源：fatal error/panic/关闭未完成时仍需 native owner 独立回收，不能调用新 machine 假装取消完成。

目前仅双机内存 transport 与 callback guard 经过测试。尚无 Windows 实际公开 pipe、host 接入或 status/stop 客户端命令，GUI 也仍只接 Linux 控制。消息鉴权必须绑定最后已读消息并恢复原线程身份，ADR-0230 的任意 PID primary-token 比较不能独自承担它。见 [ADR-0231](ADR-0231-windows-control-message-transactions.md)。

ADR-0232 新增底层 `ControlMessagePipe`：可显式创建／打开本地 `\\.\pipe\keyweave-control-<32 lowercase hex>` 消息管道，使用具体用户 DACL、非继承、异步单 flight 和明确 message read mode。它是未鉴权 I/O owner，尚未实现可信控制 transport，不是新的 Status/Stop 命令入口。不能在消息主体鉴权完成前把读出的 Request 交给业务处理器。

`close()` 是零等待回收快照；Pending 要在上层绝对 deadline 内继续 poll，取消请求不是完成。Drop 仅一次有限回退，未知飞行资源会保留到进程退出并阻止该线程继续建管道。新 Win32 调用目前通过 API mock 验证，实际 SDK/link 和真实管道验收尚未执行。见 [ADR-0232](ADR-0232-windows-control-message-pipe-owner.md)。

ADR-0233 已新增 `ControlServerTransport`，可与 `keyweave_windows::session_control::Server` 组合：完整读取后以最后消息的 thread token 鉴权，恢复原线程身份后才允许业务处理与回复。每连接只尝试一次鉴权；未读完、截断、提前或重复鉴权都拒绝，断开/新连接撤销旧授权。原始 `ControlMessagePipe` 仍是不带授权的 I/O owner。

鉴权只接受定义客户端 SQOS 对应的 SecurityIdentification，精确比较 SID、登录 LUID、session；不会因 token 查询失败回退为进程身份。恢复线程身份失败时生产进程立即终止，不能将它解释为 Stop 成功或输入恢复。当前组合链经过 API mock 测试，没有运行实际 Windows pipe/abort；客户端 server attestation、host/CLI/GUI 接入及 SDK/原生验收仍待完成。见 [ADR-0233](ADR-0233-windows-message-token-authorization.md)。

ADR-0234 已新增 `ControlClientTransport`，与原 `Client` 消息事务机组合。冷准备接收所选 host 的 endpoint 和 `ControlServerInstance`（PID＋Windows creation FILETIME）；host 可由 `ControlServerInstance::current()` 生成自身 metadata。实例提示用于拒绝陈旧目标，不是 secret 或 executable attestation。打开 pipe 前先持有预期进程 owner，实际连接的 kernel server PID 还要匹配，收发前后继续检查主体、创建时间和存活。

每 poll 仅在 NOT_FOUND/BUSY 且无未回收资源时重试一次；不等待、不拉起 host、不延长事务期限。关闭时保留进程 pin 到 pipe close 确认，`ControlClientCleanup` 分别报告 pipe 与 peer 资源事实。请求已经发出后出现错误，仍可能已经生效，不能把传输错误解释为 Stop 未执行。

目前客户端与服务端生产 adapter 都经 Win32 API mock 组合测试；Windows host/CLI/GUI 尚未接入该公开通道，host metadata 发布/发现、实际 Windows SDK/pipe 与 PID 复用/句柄转交对抗验收继续。见 [ADR-0234](ADR-0234-windows-control-client-peer-pin.md)。

ADR-0235 已在源码接入 Windows host/CLI：显式 `run-windows --control-pipe` 冷监听并发布 `pipe|PID|creation FILETIME` 的 endpoint 描述符与独立 public session。`status-windows --endpoint '...'` 和 `stop-windows --endpoint '...' --session N` 复用现有客户端 machine/pin/transport，不创建 worker。非 Windows 上明确 exit 78。

host 先监督再公开 IPC，Stop 走原 Supervisor 合作关闭，不伪造 worker progress；Starting 为 epoch0/generation unknown，只有实际进度后显示已准备代际。实际 exit 后有最多 500ms 最终服务窗，unknown exit 不制造 Exited。当前 Windows recovery_confirmed 始终 false，不能据 code0 推断恢复。重复 close 保留原绝对100ms期限，不重新续时。

目前 Windows 专属 host/driver/CLI 源码尚未由原生 compiler/type checker/link/runtime 验证；新增 Windows snapshot test 也未运行。GUI 仍待 Windows 接入，完整无输入 Windows host 链和硬件验收不能用 Linux parser/API mock 成功替代。见 [ADR-0235](ADR-0235-windows-host-public-control.md)。
