# ADR-0174：分阶段离线 CLI 与虚拟时钟回放

问题：host 可装载分阶段配置，但 keyweavectl 仍只编译/回放 Legacy，无法给后续开发提供可执行组合键和定时/reload fixtures。

选择：validate/import/replay 显式 --execution-mode legacy|staged-keyboard，默认 Legacy。编译模式与实际 controller 固定一致；schema 1 保留 inputs，schema 2 用严格 steps：input、advance、activate。schema 2 只用于 staged，禁止混用 inputs 与 steps。Advance 不是 TimerFired，不占 input seq；Activate 显式 fresh generation，冷编译同模式，旧 root 继续保旧 Arc。

虚拟时间按下一已知 deadline 精确 poll，再交付该时刻普通 input；Reset/DeviceRemoved/ContextChanged 先作为控制交付，再 drain 同刻。输入后 drain 当前时刻的实际工作；最后不自动完成未来任务，fixture 用 advance 声明 EOF 观察时间，返回真实 next_deadline。输出和转发 pointer receipts 分别保序（每个 stage step keyboard 在 pointer 前），repeat 返回目标诊断但不伪造 OS 重复。最终变量取实际 Output 生成平面。

工作与输出总量有界；时钟倒退/外部 timer token/非法 generation/断言差异拒绝。保持离线，无 capture/OS 输出。证据：真实 binary 的窗口端点/超时、duration、reload、设备控制、变量/pointer 与 malformed fixture；既有 schema 1 默认回放不改变。文档明确这是模型回放，不是平台验收。
