# ADR-0156：Simple Alone 与跨阶段修饰键快照差分

上游 `basic/event_sender.hpp:316` / `modifier_flag_manager/scoped_modifier_flags.hpp` 保存 modifier flag 集合，以 lazy 修饰键差分事件恢复，再在作用域结束发送反向事件；`manipulators/shared_event_sender.hpp:6` 将这些事件送入下一阶段。因此 Simple 快照不能直接跳过 Complex 注入 Terminal。

Simple Alone 分支使用旧 RuleSelection/Arc、严格 `< alone_timeout_ns`、同一时刻可信 context 和阶段变量条件。一个 momentary 候选与现有 Complex 行为一致：新键 Down 覆盖/清候选，变量不打断，非零 wheel 清候选；原 Up 只触发一次，Held 后仍沿原候选/超时规则，不伪造序号。

Simple wheel 的打断发生在本阶段 pointer 准入提交时，不等它在未来 Complex FIFO 交付。Complex 候选仍在其自身交付时打断；不能重复用较早的物理 wheel 在较晚交付时清掉新的 Simple 候选。

Simple 的 DownSnapshotMinusConsumed 计划在联合准入前物化差分：仅为当前不可见、保存快照中且未被消费的标准修饰键加入 lazy Down/保存 Up 槽，贯穿整个 tap 数组；这些槽经原 Complex 选择、旧 Arc 和实际输出路径执行。原计划槽号不变，新槽附加，事件和槽容量在任何登记前验证。已物化计划不再为虚构的 Terminal snapshot owner 额外计费。

上游恢复事件的 device_id 为 0，没有真实设备身份。新槽在不可伪造的私有 ActionPlan metadata 中标记为 snapshot modifier；生命周期仍归触发设备清理，但 Complex 的 per-event Device if/unless 均返回 Unknown，不能借用触发键盘 vendor/product。device_exists 仍使用独立 inventory。ProducerDecision 保存该身份事实，后续分支条件也沿用，不发明真实 device 0。

Scope FIFO 同时投影 consumed 和当前快照外 modifier 来源的实际 Complex 后代，close 恢复仍存活原来源。device/Reset 沿原来源控制清理，不复活已移除来源或计划；Alone 子批次必须等实际槽与 scope close 后才退休。静态物理根 lease 计入 Alone 动作及最多八个标准快照修饰键的 Complex 扩张。

只解除 Alone 的 actor gate；Delayed 继续拒绝，不扩大 JSON Simple schema。验证纯 Rust actor/物理控制器，不进行真实捕获/注入；Windows 本机补验仍独立待完成。
