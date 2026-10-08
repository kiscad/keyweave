# ADR-0024：有界 Linux 输入—引擎—输出执行者

状态：采用。关联 D06；独立于尚未完成的 D05 duration actor。

## 选择

Runtime 独占最多 64 个已经通过 CaptureDevice arm 的输入、每设备独立 FrameDecoder/KeyNormalizer、一个 Engine 与一个 OutputPump。CaptureDevice 保存启用时核验过的 output canonical syspath；Runtime 与唯一 backend 的身份再次比对，不能换一个 output 绕过自身排除。启动拒绝重复 id/path、非 Captured 输入、不支持的配置输出和非 Kernel repeat；配置所有动作（含 extra/delayed 与 modifiers）在输入开始处理前核验。此构造器接管已准备资源；OS 打开、consumer readiness、全体 neutral 的启用事务仍是 bootstrap 工作。

每次 poll 接收显式 MonoTime，最多读取一个 native batch，并做最多 64 次规范化/引擎/输出操作。read round-robin；未处理 raw keys 有界保留，未清空前不再读取。每个完成帧先整帧校验，MSC/LED/REP 等已公告 metadata 不进引擎，MSC 不推断 HID。SYN_DROPPED/local frame overflow 优先终止全部捕获，不让丢失帧前后不完整状态继续触发宏。frame 没完成时不能给 engine 半帧键。

所有输入、TimerFired、CancelDelayed 共用单调 seq；失败的 CancellationPending 不消耗 seq、不重复 native normalize，先分批 cancel 再处理保留事件。到期 deadline 先于普通物理输入。输出按 FIFO due 在后续引擎工作前提交；每个输出操作依然有界。实际交付时间用 caller 的 MonoTime，native timeval 不作为第二个竞争时钟；积压可能影响 wall-clock 条件，延迟/性能需真机验收。

故障、pause、输入关闭先 close 全部 capture，再 Reset engine 取消宏/计时器并清 pending/normalizers；不把未提交 intent 当已提交 up。仅 OutputPump 的 submitted 账本安排有限恢复。未知接受量立刻销毁输出；已知短写先 REPORT，再恢复 ups。最多 8 次后续恢复 poll，仍未完成就销毁 backend，不无限等待。Drop 同样先关闭输入，再销毁输出。恢复结束 Stopped，不能无条件 resume，重新启用走新 bootstrap。

读取只在 Active，失败后 poll 只运行输出收尾。last stop reason 与 progress_epoch 对外只读，无按键日志；epoch 仅在真实 poll 完成后推进，供未来 supervisor 观测。这个指标不是 watchdog 已实现的证明。hot config update 保留引擎原 binding，拒绝新配置不破坏当前活动快照。

## 未完成的范围

本执行者还没有 OS poll/时钟线程、设备发现/热插拔重新接入、桌面上下文、LED 物理同步、不可覆盖的紧急退出组合、broker/supervisor watchdog、IPC 或可抓取设备的 CLI。static 多输入容器不是原子多设备启用。D05 duration/simultaneous 的拒绝 gate 保持；不能宣称完整 Linux 后端已可部署。

离线测试使用真实 Engine 与完整 CaptureDevice/OutputPump，IO 只由故障注入源替代。成功 evdev/uinput/grab 与桌面行为待专用硬件系统验收。

## 本次验证

16 个新增运行时行为测试覆盖热更新与 repeat、双设备同键 owners、跨读取帧与 MSC、完整帧原子校验、SYN_DROPPED 优先于 due timer、held 边界、delayed up 后计时、read/EOF/pause、短写补帧/恢复、未知接受量、有限恢复预算、配置/输出身份准入、输出积压与每次工作限额、Drop/时钟故障，以及 1992 边沿 delayed cancellation 宏顺序/seq。完整 workspace 共 299 个测试通过，fmt/clippy、60-source verifier 和 Linux C UAPI probe 通过。Ubuntu 24.04.4 x86_64 / Rust 1.97.1，全部 IO 故障注入，不访问真实输入/uinput。Windows CI、实际运行循环和硬件验收未执行。
