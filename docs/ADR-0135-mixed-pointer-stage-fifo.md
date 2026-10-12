# ADR-0135：指针与动作共用阶段 FIFO

问题：ADR-0134 在未来动作尚未交付时拒绝 wheel/pointer。独立指针队列或立即透传会越过原动作数组，导致 alone 与 modifier scope 的时序错误。

选择：GeneratedStageQueue 内部工作项区分 Action/Pointer，同一个 StageTimeline 保存身份顺序、固定 FIFO 位移与设备历史。动作公开 API 保持不变；私有 mixed poll 供 owned actor 使用。指针无 producer slot、无变量赋值、无 binding owner，不进动作 delivery ledger，但占同一 8192 事件预算。TerminalWork 增 Pointer，Output 两侧共同 prepare/commit 指针；实际交付先同步当前 modifiers，再返 pointer receipt。

Simple 对 physical pointer 先排 Complex 输入队列，Complex 按交付时间更新非零 wheel 的 alone 资格，再排 Output FIFO。PhysicalStep 分别携带本步键盘 outputs 和 forwarded_pointers；同一步键盘同步 intents 必须先于 pointer 转发。每步最多一个 pointer，不丢弃接收的指针。排队时 Capture，交付时 Pass 仅表示控制器自身步骤；原生必须注入已捕获的 pointer 并做独立接受/自身输出标记，不能将排队时 Capture 当原生支持证明。

热更新不重选 pointer 配置；设备移除/Reset 丢弃尚未交付的 pointer，无假 Up。控制器移除 UnsupportedPointerScheduling gate，保留 native duration gate。实际 source pool、scope 投影、simultaneous、原生 pointer capture/injection/接受账本与 Windows/硬件仍待完成。

验证：新增4个 physical 集成测试（future wheel/Move 与 original Up/alone 顺序、old Arc/payload/source、device/Reset、仅 Terminal 有未来动作），更新2个原 pointer 行为用例。新增2个私有队列测试证明 pointer 共用8192容量但无动作槽，以及 Terminal full 时两侧 pointer 准入 abort 不推进生成 clock。完整 workspace 1247 cases / Clippy -D warnings / fmt / source60+fixture copies 通过。所有输出与 pointer receipts 由回放检查；未执行真实指针捕获/注入或 Windows 编译。见 [验证记录](validation/linux-2026-10-12-mixed-pointer-fifo.json)。
