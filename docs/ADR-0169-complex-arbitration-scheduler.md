# ADR-0169：Complex 的有界自动仲裁和回放调度

Complex 持有一个 ChordArbiter 及其首事件条件身份，另持有最多 256 个原始 replay receipts。仲裁与 replay 共用实际输入表/输出/共享组执行路径，不另建引擎。新 Down 先取消旧 Held/Alone/Delayed 候选，旧 canceled 分支先进入输出队列，最终选择用 fresh 条件/修饰视图。Ordinary/Pass 的首 Down 用仲裁器保存结果，不能再调用普通 selector；其余输入原序进入后续仲裁。

每次 admission/poll 只推进有界一步；Arbiter 自身 64-work Yield 保留 cursor，replay 一步只交付一个原 receipt。实际准入时钟用当前 actor 时间，原 event.at_ns 与原缓冲 receipt history 保持。到期 Output/initial Up/timers 先执行，不能内部 drain 并丢弃 OutputIntent。队列满或后续失败进入原 Failed/Reset 流程。

显式 frontier 表示上游尚未交付输入的真实最早事件时间。它在窗口内时先让调用方交付这些输入；暂时空队列也不允许在窗口端点等号处提前超时，等待严格晚于端点。显式 next_arbitrated_deadline/poll_arbitrated 接口协调 frontier，不把原普通 actor deadline 偷换成完整输入 EOF。

键盘之外的计划事件可以作为有界 lookahead 外部事件保存：非键变量不打断候选，其他 usage page 的 Down 按外部按键规则打断，只有键盘键可开启窗口/作为 simultaneous 成员。原普通规则优先级、条件 Unknown 和旧 Arc 保留。

Reset 取消仲裁/replay，不产生未执行 Down 的 Up。设备控制先移交待仲裁原输入，去掉失效设备输入，存活输入按原序重新仲裁；输入历史和实际重准入时间分开。默认构造及 KW-SIM/runtime gate 保留，本阶段先验收实际 Complex 的显式接口，Simple frontier、指针/Scope 混合流、完整来源预算及 Native 集成随后接入。

回放的普通 producer 保存 original_event_timing 标记；其 Alone 用原 event Down/Up 的严格跨度，避免延后至同一 poll 时钟的两个事件被当作零时长。Held/Delayed 的 setup deadline 仍从实际生成时间计算，原普通 API 的时序不改变。

frontier 不能直接用未经核验的 FIFO delivery deadline 替代：上游可能已知事件历史在窗口内，但被前面的数组/Scope 延后交付。接入 Simple 时必须查询真实未交付事件历史，确保稳定 Pending 不阻止调用方继续交付这些输入。当前显式接口使用默认 EOF 同步策略，联动入口还须传入 Simple 的实际 device idle 判定。
