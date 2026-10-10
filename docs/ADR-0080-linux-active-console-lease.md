# ADR-0080：活动控制台消费者证据与 worker 循环

日期：2026-10-10。状态：原生函数与 worker/终止报告组件已连接；实际 capture-enabled main/bootstrap 调度尚未连接。

## 改动

新增 Linux `console::Lease` 的私有单运行状态与 public ConsoleLeaseError；新增显式 opt-in `run_console_worker_controlled_report` / `run_reporting_console_worker`，接受 ADR-0079 交付的 Runtime、ConsoleProvider、同一 MonotonicClock 和 ConsoleReady。原有非 console worker 函数继续走 NoConsumer，不隐式查询 VT 或改变既有 timestamp/控制协议行为。

Console 入口再次核验 main WorkerParentGuard 与 progress 端点的非阻塞 flags；三管道/report startup 的准入仍由 ReportingStartup 执行，既有 original std FD 合约不变。不是任意 caller 可注入 fake provider 的公开控制面：通用 PollControl 为 crate 私有，生产控制台使用真实 ConsoleProvider/UinputDevice；Runtime/Pump 只增加 crate 内只读 backend 借用。

## 活动顺序与期限

1. 每轮先 receive_control。收到 Stop 直接 pause/有限恢复，不查询消费者、再读输入或等待；Stop 优先于失效证据。
2. Active 时在 Runtime.poll 前从同一 clock 重新采样，并核验证据的 owned output scope、单调时间、严格期限。首次 dispatch 必须重新查询，即使 bootstrap handoff grant 仍有效。
3. 后续证据按查询前 observed+100ms 刷新。原有250ms TTL不改变，查询前后均检查旧 grant，不能跨过旧 grant 的到期点后用新 grant 桥接空窗。新 grant 也必须在查询后仍有效，而且 observed+100ms refresh 点尚未到达；因此查询耗时达到100ms会停止，而不是把刷新点移到查询结束。
4. 任何未知、None、错误 scope、倒退、过期或 provider error 都撤销旧 grant，不能用旧 grant 继续，不能自动 rearm。错误保留具体 ConsoleLeaseError/provider errno。新的独立启动事务负责重连与 neutral 准入。
5. 成功检查后以最新实际时间执行 Runtime.poll。只有真实 poll 能增加 Runtime progress_epoch；metadata check/refresh 不改变计数或独立父监督租约。
6. wait 同时考虑 Runtime 的现有 deadline、consumer refresh/expiry 和原 housekeeping 最大20ms，选最早期限；None 额外期限保持旧 wait 行为。进入 Recovering/Stopped 后不再查询 provider，恢复路径不依赖元数据查询及时返回。

后台线程、文件监视器、自动 grant 延期或新的 wire reason 均未引入。默认 progress/KWLT v1 reason1..10 不变：消费者故障通过 WorkerError::Consumer 独立保留，Runtime 以 ControlFault 停捕获/恢复。不能把这类原生或 clock 查询错误混称成已确认的设备拔除。

## 收尾与报告

consumer error 在下一次 Runtime.poll 前产生时，通用 loop 立即 external_fault，先关闭输入，然后只走原8次有限恢复；不再 wait、publish、read 新输入或查询消费者。使用最后已接受的 Runtime 时间完成恢复，不能制造未来定时动作。原 outcome 与独立 TerminationReport 保留；没有因成功销毁输出而抹去恢复错误/未知接受量。

先采样 Runtime termination，再明确 drop Runtime（其输入先于输出），之后 drop consumer controller/provider。reporting wrapper 沿用 bounded KWLT 发送路径，清理完成后才发送终止事实；终止 frame 完整与实际 process Exit 仍须父端独立审查。入口准入失败仍返回 Err，不伪造 Runtime/report 成功。

## 验证与限制

5个纯 lease 测试覆盖首次强制刷新、100ms anchored refresh、None/错输出永久撤销、严格250ms边界、倒退、慢查询/非法窗口、provider errno保留；3个真实 Runtime + mock gate loop 测试覆盖首次失效不执行输入、活动丢失在saved up前关闭输入、Stop先于consumer query、等待期限传递、provider最后关闭和独立终止事实。它们验证状态机/loop顺序，不等于真实 VT 查询或键盘交付。

Linux Rust 1.97.1：完整 workspace all-targets 825 tests、Clippy -D warnings、fmt、60-source verifier 通过。无新增 FFI/unsafe/UAPI，本轮 C ABI oracle 未改变、未重跑。未打开真实 VT/evdev/uinput，未进行 Hook/SendInput 或驱动操作。[验证记录](validation/linux-2026-10-10-console-active.json)

每 poll 前的采样不能证明整个 poll 内无 VT/handler/会话变化，也不保证 query/poll/syscall 及时返回。前后样本不原子、不排除 ABA，handler 与 inhibited 仍是有限元数据证据；不证明端到端交付/全局恢复。独立父保护仍必要。CLOCK_MONOTONIC不计suspend，本轮没有新增睡眠/恢复保证。图形桌面/PTYS/WSL Terminal仍不属于console provider。

Windows 原生补验仍未执行：本轮实现均 Linux cfg，ADR-0073/0074/0077 的待补 Windows 检查仍受此前自动审批额度故障影响，不用旧报告代替。

下一步连接 Pending bootstrap 的 Stop/进度/有界 wait、实际 main 的 console 参数与 sealed-config handoff，再接 Linux host 的独立监督和终止证据+真实Exit。none-provider 格式和其有效配置真实 exe 测试必须永久保持无设备操作；不能用真实 capture worker 替换这些安全 fixture。完整高级规则、图形 provider、热插拔/add、LED、热配置、GUI、打包、Rust driver 与 companion 审计仍未完成。
