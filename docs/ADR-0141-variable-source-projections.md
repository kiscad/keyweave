# ADR-0141：保存变量来源的生成/实际投影

问题：映射为 set_variable/unset 的后代仍被键投影拒绝。仅隐藏键owner不能完成其保存Up/Down副作用。

选择：在已有Output投影事务内捕获原VariableAction与ActionSlot。生成与实际两侧分别跟踪引用；首引用对仍存在的变量绑定应用原Up操作，最后close对仍存在的绑定应用原Down操作。使用旧目标，不读取新配置、不做规则/条件重选。多变量同名保持既有last-assignment-wins，而不是变量owner stack。已完成/实际Up的来源不恢复；cancel/device/Reset不恢复Down。变量没有键盘输出或新增owner SourceToken，但占slot/reference预算。

同一变量slot的嵌套投影只暂停/恢复一次；逻辑Up已生成的slot不再恢复到生成视图。actual FIFO在原数组之后处理，actualUp已交付也不恢复。两侧阶段可能处于不同时间，但各自原Up后不复活。原key projection API沿用并支持变量来源，历史UnsupportedVariableProjection枚举保留兼容。

这完成Output层保存变量来源协议，仍未接Complex后代自动继承与完整Simple consumed scope；native gate保留，所有原目标继续。

验证：6个Output变量集成回放覆盖guard abort/两侧时间差/旧操作reload、嵌套与其他赋值、原Up不复活、unset/缺省Down、eager tap Up不覆盖后值、cancel/Reset；1个私有Generated full故障验证两侧变量不暂停且无ref/queue半提交。更新原Terminal变量冷拒绝用例为FIFO变量暂停/恢复。工作区1287 cases通过后，仅4处等价嵌套条件写法按Clippy合并，最终focused14 cases、Clippy -D warnings、fmt/source60+fixtures通过。最初fixture helper gen是Rust2024保留字，已改generated；没有native/硬件/Windows验证。见 [验证记录](validation/linux-2026-10-12-variable-projections.json)。
