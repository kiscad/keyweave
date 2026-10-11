# ADR-0097：Linux host 单在途 reload 事务

日期：2026-10-11。状态：原 NativeSession pump 已连接 parent reload lane；watcher、用户 reload 命令与成功硬件验收尚未完成。

## 同一会话 owner

Supervisor 新 take_reload_channel 在原单次端点 handoff 中同时交付其实际 run identity，旧 take_reload_endpoints 与之共享同一个 Option，不允许取得两份 owner。Session::new 认领该通道，创建绑定 run、初始 observed generation=1、本地5s预算的 parent lane；无 reload 模式则 None。接口不是端点认证，实际 run 隐私仍不进入 Debug/报告。

NativeSession 保留原 Supervisor、signals、clock 和 lane。每个 pump tick 仍先读取时间/信号并监督原 child，只有无 fault 且仍 Running 才推进一个 reload 状态操作，最多一次 reload IO。不会另建 worker、替换 clock、创建新 watchdog 或把配置准备作为 receipt/heartbeat。Stop/fault/actual exit 关闭 owned reload copies，原独立监督继续观察同一个 child；ObservationExpired 不丢 owner、不重置预算。

## API 与容量

`NativeSession::request_reload(Box<[u8]>)` 只入队 raw JSONC，长度1..8MiB。要求 session Running、无 fault、未 finished，以及 Supervisor 最近实际接受的 progress phase=Active；不把 startup/Standby/queued/body write 当就绪或激活。这个检查是队列 admission，实际期限在 pump 的前后 clock 检查中验证，返回 Ok 不是 worker compile/commit/ack。

一个 lane 最多一个候选和一个确认结果：Idle→Queued→Sending→AwaitingAck→Idle。新的候选在已有事务或尚未 take 的 completion 时返回 Busy，不覆盖、不 debounce、不隐式重发。Queued 独立一步以原 clock 创建 Parent.start，随后 verify_clock 做 post-check（零 IO）；Sending 分段传输，全部 local-write 接受后才消费 RequestWriter.into_completed_writer，复用同一个 owned writer。未完成/关闭的 sender 被消费时会丢弃 writer，不能得到 completion proof。

AwaitingAck 使用 ADR-0091 Parent 的严格 run/id/base/target/deadline 校验。只有完整及时 Accepted 推进 observed generation；Rejected/Cancelled 保留 base。`observed_config_generation` 查询父端已观察的代号，不能称 worker 的当前真实代号：commit 后丢 ack 时 worker 可能已变化。`take_reload_outcome` 单次取 Accepted/Rejected/Cancelled，未取前不接受下一候选。close 不回滚历史 observed generation/已合法观察的结果，也不 rearm。

## 前后 clock、监督与 Stop

每次 parent clock callback 首先读取 signalfd Stop，再采样原 MonotonicClock，要求不倒退且严格小于本次 Supervisor 原 watchdog deadline。固定 reload deadline 在 Queued 开始，prefix/body/部分ack/WouldBlock、host progress lease 的合法更新都不能延长它。queued admission 本身不开始预算，Stop 在开始前能直接丢弃候选；上层 watcher 不得无限保留未经驱动的 owner。

Stop 在 callback 中到达时锁存并关闭 lane，以同一 controller/supervisor 发一次协作 shutdown，保留原观察窗口，而非记录虚假 reload fault。信号接收/clock/IO/协议/过期错误关闭 lane并记 FaultSource::Reload，原 controller 只向 owned child 发 stop/kill；后续仍轮询同一 owner。IO/clock 系统错误保留实际 kind/errno，协议错误为 InvalidData、Expired 为 TimedOut、allocation 为 OutOfMemory。原 Control/Supervision fault 和 stop failure 分类保持独立。

若这次 Stop 驱动中已经观察到实际 Exit，立即使用已缓存真实 Report 收尾和关闭 signals，不在 actual exit 后多等待；停止观察窗口取本次原 elapsed，不因再收到信号或下一tick而延长。wait failure 也立即关闭 lane。每次 callback 最多额外读取一个 signal record，reload/body 操作仍不造 progress epoch。

## 验证与范围

Linux 1.97.1 locked/offline workspace/all-targets 931 tests、Clippy -D warnings、fmt、60项固定来源校验通过。新增5项 scripted parent lane tests：部分写/两事务复用/Accepted后Rejected/结果门控与ID/base，Idle/长度/Busy/Stop零IO，WouldBlock严格deadline，完整迟到ack不推进，Queued post-clock错误零write。新增3项 pump测试使用owned Linux pipes与 mock owner/schedule：Stop在发送前关闭零正文，EOF只停止同一 owner一次且不提前读terminal，Stop在Queued post-check到达时仍为协作shutdown且不延观察窗口。

这些 tests 不证明真实 NativeSession Active admission、真实 parent↔console worker reload 或硬件交付；实际API和新 pump 路径已编译，成功console/provider/input未执行。既有无输入actual-main harness随 workspace通过。未新增 unsafe/FFI/dependency，C ABI oracle未重跑；Windows shared transport helper与前序原生检查仍待补，host lane是Linux-only。

下一步添加实际无输入 host/worker组合 harness，验证父端真实排队/确认与原监督同时推进，再连接有界文件监听和用户reload命令。还需Windows adapter、完整高级语义、桌面/GUI/driver及硬件gate，完整目标继续未完成。见 [验证记录](validation/linux-2026-10-11-host-reload.json)。
