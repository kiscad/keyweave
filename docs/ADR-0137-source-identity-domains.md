# ADR-0137：物理 lineage 与实际 owner 的不相交 ID 域

问题：PhysicalStageController 与 Terminal 独立从小整数分配 SourceToken，虽然 lineage 不直接加入 owner 集合，同一数值会具有两种角色，无法保证整个 owned controller 内 ID 不碰撞。

选择：一个 controller 生命周期内采用两个不相交域：物理根/被捕获 pointer lineage 使用最高位1，counter 为1..2^63-1；实际 Terminal slot/snapshot owner 使用最高位0。PhysicalStageController 私有构造链配置唯一 Terminal 的 exclusive source end 为2^63；所有实际 slots/snapshot ranges 都先检查 end，再通过预备事务提交。lineage 的独占物理 cursor 和实际 owner 的独占 Terminal cursor 均单调，Reset/device remove 不复用。中间逻辑身份继续是 ActionSlot，source metadata 是原物理 lineage，不为每次重映射重新造 lineage。

独立公开 stage actor 的原构造器保留原有完整u64域，避免破坏其既有组件边界协议；只有完整 PhysicalStageController 的私有构造链承诺不相交域。此保证不跨独立 controller/process 生命周期，也不是输入或原生会话鉴权。上限耗尽显式拒绝，不切换到另一域。

替代：共享 mutable counter 会把跨 owner 的所有预备 guard 和生命周期耦合起来；分区允许既有独占 prepare/abort 保持无 ID 副作用，并给完整 controller 可证明的 ID 唯一性。全局存量预算仍由 ADR-0136 root leases 与局部队列/来源预算约束。

scope 投影、Simple advanced、simultaneous、native adapter/接受账本/Windows/硬件等原目标继续；native duration gate 保留。source 域不是 own-injected marker，不证明 OS 接受或来源鉴权。

验证：两个新私有组件边界测试分别覆盖两个slot+两snapshot来源在低域最后4个ID的prepare/abort/commit/实际owner集/后续拒绝/Reset保limit，以及物理最后一个origin ID、Reset后Down/pointer耗尽且不消费seq。原独立Terminal u64::MAX边界事务测试原样通过；一个pointer receipt断言改为高位域精确ID。Linux workspace1257 cases / Clippy -D warnings / fmt / source60+fixture copies通过，无初始失败、无原生或硬件测试。见 [验证记录](validation/linux-2026-10-12-source-domains.json)。
