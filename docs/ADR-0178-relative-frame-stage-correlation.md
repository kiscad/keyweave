# ADR-0178：相对原始帧与 stage receipt 来源关联

多轴帧和 legacy/HI_RES 不能拆成数个会重复输出的 pointer 原输入，也不能压缩成虚假的滚轮单位。选择 OS 无关 PointerActivity{moved,scrolled}，作为一次原生帧的语义资格摘要，不携带原始值；保留原始 RelativeFrame 在 Linux 有界 PendingRelativeFrames 中。stage Activity receipt 与 Wheel/Move 共用真实 FIFO、modifier sync；scrolled 取消 Alone，moved/零摘要不取消。

PhysicalStep.registered_pointer 返回准入成功的 high-domain SourceToken，和以后 forwarded receipt 同源。Reset 不重用 source，injected/失败不建立映射。缓存 register 在此 source 下持完整设备、原时间、旧 Arc、Frame；原始值/顺序无变换。take 先核 source/device/Arc/activity，history>=原时间、delivery>=history，成功一次取出。history 可被 stage FIFO 归一，不能错把原时间相等当 receipt 事实。元数据关联不是鉴权/原生接受证明。

缓存总8192 records/4096 frames，can_accept 在送原输入前检查，register 再核且 source 严格递增。discard_device/discard_older/discard_all 只丢缓存并保 source cursor，不为取消帧发送空 report/Up。活动 Native owner 必须与实际 controller 冷预检/准入/注册在同一顺序执行；未来 Runtime 包括原始 packet 拆分、输出 receipt取回和消费授权，不能从本缓存推出完整捕获已完成。

默认 Windows/Linux keyboard machine 明确拒绝 PointerActivity；CLI可离线显示摘要。Legacy model 采用同样 Alone/sync语义，不输出原生 relative。来源预算仍使用既有pointer来源域；缓存不另分配Source。证据：实际 controller 与 cache 的原帧保存、timed FIFO/旧Arc、wheel/move资格、撤销、malformed receipt 与容量冷拒绝。无依赖/unsafe/FFI新增。
