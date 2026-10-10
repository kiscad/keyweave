# ADR-0067：Linux 精确实例移除控制事务

Runtime此前只有整体pause/fault路径，无法对可信生命周期事件完成单设备移除。增加remove_device(device, now)：要求active、完整DeviceInstanceId/generation精确匹配当前captured实例；未知/旧代/重复请求拒绝且不推进时间/seq/修改其他资源。可信controller必须在普通poll/due工作前交付事件；本API不是物理拔除检测或事件来源鉴权。

有效事务先关闭该唯一capture client，再清其decoder/normalizer/emergency、丢弃raw/logical中该slot的未交付事件、移除captured metadata，并交付Engine::DeviceRemoved释放仅其owners与撤销其timers/macros（不运行alone/after-up/delayed分支）。输入Vec保留closed tombstone，slot不重排/复用；round-robin只读live captured slot。其他已排队事件保持原slot/顺序，输出已逻辑交付的FIFO不任意删除；新的up仍经同OutputPump，不直接注入或清他人的submitted状态。报告只确认控制事务状态，不证明输出up已接受/全局恢复；progress_epoch不因控制增加。

剩余captured设备至少一个且可产生emergency chord时继续active；最后设备或最后可达emergency接口移除后先close全部输入再进入原有限恢复，明确NoDevices/EmergencyUnavailable原因，不自动恢复/重插。Engine或output enqueue失败保存原错误并整体stop，不能留下部分失败事务继续capture。partial inventory通过原bounded ContextChanged下一poll刷新，Engine本次控制已退休该identity，不把局部absence当全系统absence。

普通read/EOF/ENODEV与native wait HUP/ERR/NVAL仍走原整体fault路径；不得靠errno推测物理拔除。Linux v6.18 evdev_read与poll同时覆盖!exist和client->revoked，后者也唤醒HUP/ERR：[内核源码](https://raw.githubusercontent.com/torvalds/linux/v6.18/drivers/input/evdev.c)。原生热插拔provider、可信generation与身份事件绑定、device add/rearm/consumer-ready仍需后续实现。本轮只实现必要runtime控制，不称自动热插拔已完成。

验收：双设备同键不提前up、仅释放唯一owner、移除先于同刻held/delayed到期、旧热配置binding正确释放、不生成alone/after-up、已交付与未交付事件区分、slot tombstone/round robin、unknown/stale/double/clock请求、最后设备/emergency失去与输出短返回恢复；全部mock，不访问系统设备。

软件验证：Linux 728 tests 与独立无设备 worker_parent harness；Windows x64/x86 各702 tests、Clippy/fmt/既有SDK通过。[最终报告](validation/windows-2026-10-10-device-removal.json)。初次一个错误mock receipt失败已保留，修正fixture而未放宽生产契约。
