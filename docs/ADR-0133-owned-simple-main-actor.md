# ADR-0133：拥有状态的 Simple main actor

问题：Simple 父 binding 与 Complex 生成/实际输入仍需要外部测试逐项驱动，不能作为流水线组件使用。

选择：SimpleStageActor 私有拥有 Simple 输入原计划/producer arena、Complex 生成队列、ComplexStageActor 和保存的孩子 Up 凭据。Down 一次选择旧规则，并使用 ADR-0132 guard 联合激活/登记/生成；原 Up 先联合预检父标记与保存的生成 Up，再提交。实际 Complex producer 完成才减孩子 remaining；原 Up 已提交且所有实际派生孩子结束后，退休生成队列和外部 child 登记，再 finish Simple 父/退休根。

poll 每次只交付一个有界阶段步骤，返所有输出 intents；先 drain 到期下游，再交付一个 Complex 输入。timer/context 采用调用方本次显式时间/上下文。生成变量视图与实际交付保持分离。跨 owner reload 先全部预检，保存父继续旧快照。

device/remove/Reset 控制先回收实际输出，再清生成队列和父树；不调用 alone/delayed 退出动作。错误锁存 Failed，只有 DeadlinePending 允许先 poll 后重试；Reset 是恢复 intents，不证明 OS 接受。batch IDs 从当前所有 owner 已用最大值后分配，Reset 不重用。

Simple 根未释放槽（包括未来 Reserved）参与下游 EOF 同步谓词；空 Simple Up 也可显式同步已无活动后代的 lazy modifier。这是登记的 Simple 输入组生命周期，仍需物理入口保证 root 分组和实际设备来源。

边界：本 actor 实现 Simple main（上游 simple 配置形式）。Simple 高级分支和 consumed modifier scope 投影尚未实现，必须显式拒绝而非静默错映射。Complex 分支由已有 actor 处理。仍保留 native duration gate；未接真实物理入口、统一 source pool、simultaneous、GUI/安装/驱动。不能据纯组件测试宣称全移植完成。

替代：暴露 mutable arena/队列会继续要求调用方配对所有状态和退休；采用私有 owner。复用同步 Engine 会丢失已规划的跨阶段 duration，因此不采用。

验证：11 个新增集成测试覆盖两阶段旧选择/热更新、future Simple duration、Complex held 同刻原 Up 与 delayed 原 Up 后保父、变量生成/交付差异、空消费、Reserved Simple EOF/空末尾显式同步、两设备移除/Reset、DeadlinePending 和 Failed。工作区 1225 cases 通过，最终 Clippy/fmt/source60 通过；完整 suite 后仅一处等价测试断言格式修正，focused 11 cases 再通过。初轮夹具 API 和 FIFO 预期错误未改变生产时序，见 [验证记录](validation/linux-2026-10-12-simple-main-actor.json)。
