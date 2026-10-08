# ADR-0003：跨阶段动作序列保留事件生命周期

状态：已实施基本键动作；2026-10-08。

问题：simple `a → [b,c]`、complex `b → [d,e]` 不能直接展开成 `[d,e,c]`。上游事件阶段接收的是 b down/up，再接收 c down；d、e 都须在 c down 之前释放。直接拼接数组无法扩展到各阶段的 up 动作与轻触判断。

选择：引擎依次按阶段执行每个动作，前面的动作完成 down/up，最后一个动作将持有的输出交还物理 binding。每个最终输出生命周期分配独立 SourceToken。匹配顺序保持数组首匹配，输出只进入后续阶段一次。物理重复不重新播放序列，松开沿旧 binding 释放，不重新查当前配置。

证据：固定上游 `src/share/manipulator/manipulators/basic/event_sender.hpp` 的 `post_events_at_key_down`，条件 `it != end - 1 || !to->get_repeat()` 决定即时 up；最后 repeat 动作登记在 `events_at_key_up`。当前仅实现默认 repeat 键动作；`repeat:false`、修饰、hold_down 等选项仍准确拒绝，不能宣称完整实现该函数。

边界：每次新 down 在状态改变之前计算跨阶段保守展开上界，最多 512 个叶动作/1024 个输出边沿；整个引擎最多 16384 个 owner、4096 个物理 binding。释放/Reset 必须完成全部既存 owner 清理，不截断成新 down 的限额。编译器发现配置超出单次展开上限会拒绝无损激活；直接构造 IR 的调用者仍受运行时检查。

冲突：序列轻触与其他来源持有同一输出键时，遵守 ADR-0002 的所有权策略，不发送会打断他人持键的 up。该策略是明确的 KeyWeave 行为，不冒充已逐字节复现上游。离线黄金 `tests/fixtures/sequence.json` 与引擎多来源测试固定行为。

替代：拼接动作数组实现更简单，但会丢弃中间绑定的 up 生命周期，拒绝采用。后续高级动作在阶段绑定中保存规则与旧快照，不能回退成扁平宏队列。
