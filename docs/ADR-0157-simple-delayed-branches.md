# ADR-0157：Simple Delayed 保存父绑定与分支

Simple 新增单个 `(checked deadline, ActionSlot)` Delayed 候选。原 Up 不取消它，父绑定和物理来源 lease 等候选触发/取消及实际孩子完成后才回收。新 Down 先取出旧候选，生成旧规则的 canceled 分支，再用生成后的变量/修饰视图选择新规则；候选取出一次，不重放 canceled/invoked。Wheel 只影响 Alone，保持既有 Complex Delayed 规则。

poll 先处理 due 下游/生成工作，再选较早 Held/Delayed deadline；相同 deadline Held 优先。invoked 在实际 poll 时间生成；所有 tap 分支经 ADR-0156 快照差分、原 Arc/device 生命周期、未知合成事件身份和联合子批次准入。空/条件过滤分支仍终结候选；父在原 Up 已提交且全部孩子/scope 完成后回收。

公共阶段校验接受 Simple Delayed，Legacy Engine 仍拒绝；静态来源 lease 纳入 invoked/canceled 动作与各自八标准快照修饰键扩张。设备移除/Reset 清候选、保 ID/source 单调；失败仍 Failed/reset，不承诺整图回滚。此后 Simple actor 的 main/Held/Alone/AfterUp/Delayed 均可执行，但不扩大 JSON Simple schema。simultaneous、native pointer、平台/硬件、GUI/包装/driver 和整体目标继续。
