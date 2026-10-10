# ADR-0070：Linux 终止事实保留

Runtime此前销毁输出后清空 ledger，StopReason/Stopped 因而不能区分实际接受的恢复、REPORT未完成、未知接受量和有限预算耗尽。新增只读 TerminationReport，在唯一 backend owner 释放之前采样输出状态，之后独立记录 owner释放与 ledger transition。保存完整停止原因、输入owner释放、Engine reset、恢复错误、预算耗尽、未释放submitted、pending REPORT、未知接受量和输出恢复确认等事实，不包含按键/设备/path/run身份。

恢复确认只在正常 Recovery phase、已知empty submitted、无恢复错误且无pending report时成立；destroy_backend清账本不能制造确认。独立Engine与owner结果不能被 output确认覆盖。报告是进程内资源/接受量事实，不是compositor已处理、全局键状态或物理ungrab API成功的证明；输出owner dropped不等于消费者已接收up。Drop回退不产生可读取的成功报告；Active/Recovering直到明确销毁前仍为None。

新增受控 worker 报告入口，保存原Result（主循环/进度/恢复错误）及Runtime终止报告；旧StopReason接口保留原行为。父保护/端点准入失败仍在报告之前拒绝，不伪造shutdown。停止报告不经progress帧或退出码截断交付，独立跨进程通道与真实Linux可执行启动仍待接通。

验收：active/pending无最终报告、正常up/REPORT完成、未知空前缀接受量、恢复错误、预算耗尽pending report/held key、重复读取与旧报告稳定、worker Stop成功和独立错误并存。纯mock，无真实输入/driver操作。

最终验证：Linux756 tests及独立无设备harness；Windows x64/x86各716 tests、Clippy/fmt/既有SDK通过。新增6可移植Runtime+2 Linux-only worker报告tests。[报告](validation/windows-2026-10-10-linux-termination.json)。无本轮失败，无真实全局输入/设备操作。
