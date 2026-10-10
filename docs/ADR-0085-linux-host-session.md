# ADR-0085：Linux host 的 owned 会话控制与终止分类

日期：2026-10-10。状态：控制组件已实现；原生驱动循环与 CLI 待连接。

## 决策与控制顺序

`keyweave_host::linux::Session` 消费并持有已启动的同一个 `Supervisor`。每步显式输入与 spawn 同域的 `MonoTime` 及可选 StopSignal；不得让 capture 线程驱动自己的监督 lease。调用方仍须保证可信 controlled worker 和独立、及时的 host 控制线程。

首信号锁存，只有第一次调用 `request_shutdown`，然后正常 poll 原 owner；后续信号不会重发请求或延长 shutdown deadline。信号不是 progress，不能续租。Supervisor 原有 partial-command、Stop 顺序、lease、超时 kill 和实际 try_wait 语义不变。

原生 signal/clock/wait 故障由 `Session::fail` 进入；监督操作错误自动进入首故障。保留主故障来源和独立的首 stop failure，调用 owned `request_stop`，并继续 poll 同一个 owner。强制停机请求或接受不代表已退出；后续 owned Supervisor poll 继续重试其 kill/reap 行为。不能用 Drop 或观察超时替代 actual exit。

只有 `SupervisorStatus::Exited` 才调用 `collect_terminal`，保存实际 ExitStatus、监督终止原因、首信号、主故障、独立 KWLT 观察结果与 epoch；随后缓存结果，重复 poll 不做 IO、重置预算或发信号。没有实际 exit 时绝不读取终止 frame，也不产生终止报告。

## 分类边界

`recovery_confirmed` 要求完整 KWLT、观察无故障，以及全部独立 owner/recovery facts 满足：input owners released、engine reset、无 recovery failure/budget exhaustion、无 submitted/report pending/unknown acceptance、output recovery、output owner dropped 与 backend transition 确认。

它只是 worker 报告事实的分类，不认证全局键盘状态、硬件接受、driver 健康或进程退出。`successful` 另要求实际 code=0、监督原因 None/Cooperative、终止原因 Pause/Emergency、无 host 故障。普通 fault 即使恢复确认仍不算成功；缺失 KWLT，即使 code=0 也不算恢复确认。pre-Runtime Stop 尚没有独立可证明的 no-dispatch host 分类，缺帧不自动视为全局恢复。

## 验证

见 [验证记录](validation/linux-2026-10-10-host-session.json)。四个单元测试使用私有 Owner seam 验一次请求、退出前零终止读取、退出后缓存、独立主/停机故障与退出/恢复分类。

新增 actual-main 无输入 harness 的三个 child 模式使用父死亡/独立组 guard、sealed fixture 配置和真实三管道，读取 Stop id=1，分别输出完整、恢复不完整和缺失 synthetic KWLT。host 保留首 Interrupt、实际 code0、Cooperative、epoch0，正确区分三种 report；后续 Terminate/Hangup 不覆盖首事件或重做观察。所有控制事件纯注入，未发送 OS 信号。fixture 事实仅验证传输与分类，不是实际 Runtime 或硬件恢复。

全套 Linux tests、Clippy、fmt、上游60来源检查通过。无新 unsafe/FFI，C oracle 本切片未重跑；仅添加已有本地 keyweave-linux 的 Linux-specific host dependency 和对应 lock 引用。Windows 前序原生补验仍待执行。

## 下一步

连接 HostSignals 与同一个 MonotonicClock 的原生有界等待/控制 pump，明确 fault 后 reap 的有限观察和不完整结果；再把 cold config、run identity、worker locator、argv、sealed-config isolated spawn 组合为 `run-linux` CLI。保持 None 有效配置 exe 测试永久无设备，真实 console 成功路径不得自动运行。整个跨平台开发目标仍未完成。
