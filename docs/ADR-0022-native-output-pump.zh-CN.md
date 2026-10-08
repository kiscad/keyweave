# ADR-0022：native 帧完成与 submitted 账本适配

状态：采用。关联 D06 与 ADR-0019/0021；真实 capture/runtime 尚未接入。

## 问题

OutputQueue 原有 acknowledge 只有 accepted 边沿计数。Linux 写入可能接受整批 EV_KEY 而缺少 SYN_REPORT；单看数量会保留尚未执行的后续宏并继续普通输出。未知接受量则不能清空 flight 或假定提交了零边沿。已有 uinput FD 与 queue 尚未连接，需要一个有界、可离线故障注入的执行者。

## 选择

OutputQueue 增加 acknowledge_incomplete：验证同一个 flight ID 和 accepted 范围，保存已确认的 prefix，且无论数量是否等于整批都走恢复。普通提交失败会丢弃未来 intent，只按 submitted set 安排 up；恢复提交的短 prefix 则只保留未接受的 up。REPORT gate 由平台执行者持有，账本 Paused/neutral 本身不能证明 REPORT 已完成。

增加 Quarantined mode：保留 flight、已知 submitted prefix 与队列用于诊断，禁止 enqueue、resume、begin_due、acknowledge、begin_recovery。backend_destroyed 必须在真实输出设备销毁后调用，才清除未知 flight 和所有键状态，并回到 Paused。submission ID 单调序列保留，旧 ack 不能匹配后续新提交。submitted 在 Quarantined 状态只代表最后已知状态，不是当前内核状态的完整描述。

`keyweave-linux::pump::OutputPump<B>` 持有唯一 backend 与不可从外部修改的 queue。enqueue 在修改账本前完整转换 HID→canonical native，并验证公告 capabilities/repeat；数量先检查，转换和队列都有界。每次 poll 只执行一次最多 256 边沿的提交，或一次 REPORT flush，不做无限重试。

阶段为 Normal、ReportPending、Recovery、Quarantined、Detached。完整 KEY prefix 但帧不完整时，先 acknowledge_incomplete，再进入 ReportPending；下一次 poll 只 flush report。flush 成功才进入 Recovery。之后提交账本所需 ups；恢复即使变 neutral 也不回 Normal。空 submitted set 同样不能跳过 REPORT。poll/恢复操作检查显式单调时钟，REPORT 分支也不能绕过检查。

FrameBackend 的外层 io::Error 契约是写入前拒绝，接受量已知为零；写入后无法确认的错误必须使用 UnknownAcceptance。数量/完整性违反契约的 receipt 同样 quarantine，不能猜 prefix。UinputDevice 是 production 实现，fd 不泄露；mock 只用于行为故障注入。

begin_recovery 供 runtime 停止 capture 后使用。uinput release_submitted 同时允许 Ready→Recovery 的主动收尾，但仅接受 up 且先验证 capabilities；短写/REPORT/quarantine 的门槛保持。这个变化使正常停止也可以释放原先成功提交的键。

`destroy_backend` 先冻结账本、drop 唯一 B（UinputDevice Drop 销毁设备和关闭 FD），然后清账本并进入 Detached。Detached pump 无 attach/resume；runtime 重启时重新建立输出和全部准入 gates。不靠销毁前伪造 zero ack，也不复用原虚拟设备恢复抓取。

## 未实现范围

本组件可以执行引擎输出，但没有 evdev frame→Engine 输入线程、grab/broker、消费者 ready、物理 neutral、own-device 排除、watchdog、deadline 驱动和热插拔。resume 是带明确调用约束的入口，不是这些 gates 已实现的证据。暂停/错误后的物理 capture 必须由后续 runtime 即时释放。Quarantined ledger 不可用于推测全部未知键并无期限补 up。

## 验证

OutputQueue 两个新测试覆盖全边沿但不完整帧、未知接受量冻结、销毁后清理和 stale ack。Linux pump 十个测试覆盖完整前缀补 REPORT、空账本待 REPORT、未知 errno、REPORT 错误、capability/alias/repeat 准入、非法 receipt、写前拒绝、主动恢复、单调时钟，以及真实 Engine 持键热更新→旧映射 native up。使用可记录调用顺序的 backend，不创建系统设备。ABI、源码 hash 与 workspace 检查沿用既有工具。
