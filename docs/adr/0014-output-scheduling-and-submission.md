# ADR-0014：有界输出时间线与 submitted 账本

状态：调度层已实现、离线验证；hold duration/运行时接入未完成。2026-10-08。

## 问题与来源

固定基线的 `basic/event_sender.hpp:224` 在立即轻触 up 前累加 hold duration；最后 repeat=true 的动作只保存原输入 up，并不按该时长自动释放。`shared_event_sender.hpp:69` 对 alone 等完整轻触同样累加。`types/to_event_definition.hpp:47` 接受 hold_down_milliseconds 和 held_down_milliseconds 别名。三个文件的 hash 已在 source manifest 中。上游 Mach absolute_time 的单步自增不直接变成纳秒 oracle。

仅在 OutputIntent.at_ns 上加时长，随后立刻对未来输入改 owners，会跳过后续映射阶段的时间顺序；仅使用 held timer 也不能表达一个数组的后续动作等待。必须同时规划阶段事件的时间线与后端提交顺序。当前引擎尚未接入 duration，继续准确拒绝该配置字段。

## 选择

新增独立纯 Rust `keyweave-output`，接收末端 OutputIntent，按 FIFO 的 earliest dispatch timestamp 排队。后续较早时间的输入输出不能越过先前已排队轻触的未来 up；等时刻保持数组顺序。调用者显式注入单调时间，不睡眠、访问 OS 或拥有输入抓取权。

每批 append 最多 1024 边沿，总量 8192、desired/submitted 按下键最多 4096、每次提交 256；整批预检所有中间状态与容量，失败不改变队列和 desired。一个 submission 在途，递增 token 不重用；可以继续有界 append，但禁止并发提交或在接受数量未知时 reset。backend 报告已接受的有序语义边沿前缀；字节数、native packet 数不得未经转换传入。

账本分离计划最终 desired 与 API 实际接受的 submitted。acknowledge 只提交已接受前缀；普通短返回清除所有未来计划，只对仍在 submitted 的 down 排 immediate up，进入 Recovering。恢复短返回保留剩余 up 供重试；全部完成进入 Paused，不自动重新抓取。初始也 Paused，resume 需输出 neutral，物理 neutral/capture gate 由调用者负责。实际 API 接受与目标应用收到仍不同，未声称 confirmed。

## 边界与后续工作

这是末端调度组件，不是完整 hold duration，也没有 evdev/uinput/SendInput。后续需有界阶段事件队列，使 simple→complex 派生输入在其真实计划时间执行，决定同刻 timer/context/物理事件的顺序，并在设备移除/Reset 取消未提交阶段事件。接入运行时需要协调 engine Reset、关闭 capture 与 output recovery；不同设备 owner 汇聚后，末端 OutputIntent 没有 device 身份，禁止在这里猜测设备并单独取消其输出。

backend 写入接受数量不明确时必须隔离并恢复物理输入，不能伪造 accepted=0；永久输出故障不能无限等待 up 才恢复物理输入。这里的 recover 重试接口不代替 watchdog/租约/故障策略。Windows UIPI/安全桌面或设备失联仍需各平台处理。

11 个调度测试与 1 个 engine 联合测试覆盖未来 up/FIFO、部分接受的全部前缀、失败丢弃新 append、恢复重试、实际 down 清理、在途/旧 token、单调时钟、repeat、队列/按下键容量与 atomic rejection。暂不连接 CLI replay，保持现有 EngineState 的 intent 语义明确。
