# ADR-0134：纯物理输入阶段控制器

问题：阶段 actor 接收原计划 receipts，但原生入口只有标准化 InputEnvelope；缺少物理来源、保存原 Up 与一条物理 Repeat 沿两级保存主绑定查询的拥有者。

选择：PhysicalStageController 私有拥有 SimpleStageActor、每个 (DeviceInstanceId, KeyId) 的物理根计划、原 Up 后仍 pending 的 root slot、单调 source cursor、设备墓碑和 context。新 Down 构造单槽 Simple 根并只选择一次；Up 使用原计划保存的 Arc/source/slot，不读新配置。重复 Down、未知 Up/Repeat 不造 owner。Repeat 只返当前实际保存主叶的 target，绝不重放动作数组或直接制造 Repeat intent；backend 仍必须证明 desired/submitted，并选择单一 repeat 来源。

InputEnvelope seq 严格递增，时间单调；到期阶段先由 poll drain，普通输入遇到 DeadlinePending 不消费 seq/设备/来源/规则。设备世代观察先释放旧实际绑定，再丢弃对应物理根。Reset 在普通 due 之前处理，不触发用户退出分支，保留设备墓碑/ID/source cursor/seq。Injected 输入只 Pass，不产生物理绑定。context 使用与原 Engine 共享的 bounded/revision/TTL/设备墓碑验证；timer 新条件读取最新 context。所有时间显式注入，无 OS API。

wheel/pointer 转发和 modifier 同步入口、阶段内部 timers、错误锁存均需验证。poll 每次只返一个 bounded step；不能内部无限 drain 或丢弃输出。控制器实际错误需要停捕获并 Reset；Reset 的 intents 不是输出接受或全局恢复证明。

外部 TimerFired/CancelDelayed token 属于旧 Engine 调度接口；新 controller 仅使用自有 next_deadline/poll，不接受伪造 token。尚未替换 native runtime，native duration gate 保留。Simple consumed scope 投影/高级分支、全局实际来源预算、simultaneous、Windows 原生补验/硬件/GUI/包装/驱动继续待做。

指针顺序边界：当 Complex 生成队列或 Output 实际队列仍有未来工作时，wheel/pointer 在 seq/device/source/条件变更前返回 UnsupportedPointerScheduling，避免越过原数组导致 alone/scope 时间语义错误。Complex 的未来 held/delayed timer 本身不作为排队动作，当前 idle-queue pointer/alone 路径仍可用。必须后续接混合 action/pointer FIFO 才能解除此 gate。普通 native Engine/runtime 未切换。

验证：15 集成+1 来源耗尽 unit 覆盖两级旧绑定/Repeat/Up、duplicate/unknown、seq/due/time cold retry、代际替换与共享 key owners、Reset 先于 due、delayed pending 与同键新 Down 取消旧 task、held/tap/modifier repeat eligibility、Injected Pass、fresh context timer、pointer/wheel/lazy、伪造 token、future pointer gate 和溢出 Failed 恢复。Linux 工作区 1241 cases / Clippy -D warnings / fmt / 上游 source60+fixtures 通过；无 native/hardware/Windows 验证。见 [验证记录](validation/linux-2026-10-12-physical-stage-controller.json)。
