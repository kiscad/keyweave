# ADR-0136：物理根跨阶段来源预留

问题：不同 arena/生成账本各自限制 16384，不能据局部容量宣称全流水线来源预算。相同 ActionSlot 的上下游登记不能重复计数；还要为原 Up 后的实际 delayed/持续时间孩子保留容量。

选择：PhysicalStageController 对每个新物理根预留其旧快照的静态跨阶段上界。Simple 根计1；每个 Simple main 展开目标/附加 modifier 计 Complex 输入槽1及其所有可能 Complex 输出槽；variable main 也计 Complex/Output 两槽。Complex main/held/alone/after/delayed 两候选数组均纳入，snapshot 分支各额外预留8标准 modifier 来源。条件/数组过滤只减少实际使用；匹配候选取最大值，并纳入 pass-through。整个 lease 保留到实际 Simple producer 结束或设备/Reset 撤销，原 Up/中间孩子结束不能提前归还。

全局总 lease 不超过16384；超限在 seq/device/source/规则/输入队列变化前拒绝。热更新只更换新根估算缓存，不改已有 lease；新设备代际准入可扣除将由控制回收的旧代 lease。duplicate Down/unknown Up/Repeat 不分配。静态估计以当前支持的 Simple main 为前提，Simple advanced/scope 投影仍拒绝。

此为实际控制器全局准入预算，并非统一实际 SourceToken ID allocator。原 Simple/Complex/Terminal arena 的局部预算仍保留；该 lease 预算包括未使用的可能分支，可能较精确峰值更保守。队列容量、distinct key/suppression reference/输出接受仍各自检查，不承诺任意宏一定准入成功。native gate 不解除。

具体 scope 策略：held/after_key_up 是 CurrentMinusConsumed，无新增 snapshot owner；alone 与 delayed 两候选采用 DownSnapshotMinusConsumed，各按最大8标准 flag预留。静态候选可能被条件/halt/filter跳过，仍保留上界直到整个根退休。

验证：4估算测试覆盖变量/中间槽、所有分支/3份snapshot、候选最大/fallback以及modifier目标的Complex展开；4控制器测试覆盖8705-unit root的超限cold拒绝与原Up后保留、reload旧lease、新设备代际回收、delayed实际tap结束释放。完整Linux workspace 1255 cases、Clippy -D warnings、fmt、source60+fixtures通过。原 tests/source_budget.rs 恢复并比对HEAD bytes，新测试在timed_source_budget.rs；无原测试删除。见 [验证记录](validation/linux-2026-10-12-root-source-leases.json)。
