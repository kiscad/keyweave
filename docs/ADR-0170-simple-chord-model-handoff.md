# ADR-0170：Simple 到自动 Complex 仲裁的完整引擎模型交接

Simple 的显式 new_with_chord_model 构造同一个现有 actor/生成队列/Complex actor，不生成删去 simultaneous 的伪配置。该模型接受完整有界 Complex simultaneous metadata，Simple simultaneous 仍拒绝。默认构造、Physical/Native 工厂和 compiler KW-SIM gate 保持严格，不能把模型回放验收当成完整来源预算或原生平台验收。

生成队列角色固定到生命周期结束：构造、旧 child plan、下游决定和热更新使用相同角色校验。模型热更新不能切回另一条执行路径，旧计划保留原 Arc。Complex 仲裁 admission/poll 接收 Simple 的实际 device_idle/EOF 判断。

timeline.rs 的 PreparedStageAppend.commit 将归一化后的时间写入实际排队 event.at_ns；next_deadline 返回队首同一字段。GeneratedStageQueue 的下一事件时间因此是真实未交付的阶段事件 history frontier，不是当前 poll clock。Simple 模型使用此 frontier 计算 Complex wakeup；稳定 Pending 不挡住窗口内已排队输入。到期下游输出/initial Up/timer 保持原优先级，随后一条生成事件或一条回放；所有 OutputIntent 保留。

Scope 命令维持既有 FIFO，允许捕获仍 Reserved 的成员模板，匹配准入时实际共享投影。Pointer 用现有输出交接；非零 wheel 的原事件时间记入 Alone 干扰历史，晚交付或匹配后不能忘记等待期间 wheel，纯移动不打断 Alone。Reset 清除历史，控制不制造原 Up。

随后须完成 Physical 全局来源预算/统一模式入口、原生 pointer/repeat 与 Windows/Linux 验收，才能解除生产 gate。

完整回放发现 Scope close 可能越过仲裁器尚未执行的 body 输入。模型将这种 close 保存为独立有界队列，继续交付后续匹配输入；body 的原 Up 已实际进入 Complex 后才关闭，而不是等待共享组所有成员退休。这样 all 模式下最后成员尚未 Up 时也可恢复共享输出。待 close 时阻止提前退休 Simple child，处理完成后再走原退休路径；设备控制/Reset 显式清理，所有事件 history 不改。计入 pending work，最多 MAX_BINDINGS 条。

测试中的定时数组按既有 FIFO 将其后生成 b 的阶段 event 时间归一化到数组末尾；回放按此实际阶段时间而非原物理登记时间判定窗口。Scope 用例因此使用覆盖归一化时间的阈值，不改上游 fixture，也不靠内部丢弃输出调整顺序。
