# ADR-0155：Simple Held 保存子批次替换

Simple actor 用一个保存 `(deadline, ActionSlot)` 的 Held 候选，沿用既有 Complex/上游单候选行为。合法新 Down 覆盖或取消前一候选；原候选 Up 取消候选；deadline checked-add 在 Down 登记前验证。poll 先处理到期下游和生成工作，再以本次实际 poll 时间生成 Held，不伪造输入序号/TimerToken。

触发时先提交旧 main 的保存 Up 并标记该孩子已释放，再由冻结父规则选择 Held（动作条件用当下生成视图/context），联合登记新的 Complex 子批次。bindings 指向新 main/repeat；旧孩子可继续执行保存数组并按 FIFO 完成。Held 非 tap 子批次等原物理 Up，repeat 跟随新保存主叶；父只有全部孩子与 consumed scope 完成后回收。空/条件过滤掉的 Held 同样释放旧 main，不复活它。

来源 lease 包含 Simple Held 的 action/modifier 和 Complex 扩张；state/next_deadline 计入真实待处理候选。设备移除、Reset 清候选，ID/source 不重用。保留 Alone/Delayed gate；不扩大 JSON Simple schema，不改 native entry/输出接受账本。本轮纯 Rust actor/物理控制器测试，不捕获或注入真实键盘。

公共快照校验原来同时禁止 Legacy 和阶段组件的 Simple Held。本轮只在既有 allow_action_timing 阶段校验路径接受 Held；Legacy Engine 仍拒绝，Delayed 公共 gate 保留。测试须验证这一分界，不能把打开局部 actor gate 当完整准入。
