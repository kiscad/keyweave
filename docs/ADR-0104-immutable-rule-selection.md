# ADR-0104：不可变阶段规则选择与主动作计划

日期：2026-10-11。状态：共享选择器已接入现有Engine，另提供下一阶段主动作计划；完整deferred actor、原生duration与simultaneous入口继续待实现。

## 选择结果

新增rule_selection::MappingStage仅包含Simple/Complex，RuleSelection持有原Arc<ConfigSnapshot>、阶段/index、按下时modifier集合、实际消费modifier侧别和一次过滤后的主to动作索引。字段私有，mapping/actions只返回对应旧snapshot里的引用，不能用新配置数组解释旧index；没有Clone/Debug或原生handle/epoch生成。

select_rule只扫描指定阶段，保持原数组first-match顺序：from相等→规则conditions全部Truth::True→modifier consumed成功。Unknown不能因unless变成已知匹配；调用者必须提供已有三值语义的评估器。找到规则后一次过滤整个主to数组，保存索引；即使结果为空也保留这条匹配，不向后面的规则回退。消费集合直接保存，不在变量动作执行后重新选择左右modifier。

这是可信调用方的纯状态决策API，要求validated immutable配置和同一生成事件的condition state；它不验证native lease、预留source/slot、创建binding或执行动作。公开评估闭包必须在这次调用中使用一致状态，任意caller构造的配置/True结果不是原生权限证明。输入大小/allocator/评估时间没有新增硬实时承诺，原引擎冷校验和扩展预算仍负责实际准入。

## 接入现有引擎

Engine.condition_truth统一原变量特殊规则与普通context三值结果；变量缺失仍按Integer(0)判断，unless保持严格值类型语义。conditions_match和新selector复用此入口。press_stage继续先执行原该阶段delayed cancellation，再按当前状态选择；terminal分支仍只建立原Leaf/Suppressed所有权。

main-to复用RuleSelection.actions，不再分别为last modifier和主动作遍历重复评估条件。press_filtered_actions使用这次生成事件冻结的数组，原modifier hide/unhide、scope、repeat、variable、halt和所有权动作保持原路径。held/alone/after-up/delayed分支仍在各自生成事件过滤，不能复用最初main-to的eligible数组。

相关私有函数传递&Arc<ConfigSnapshot>，选择器可保留实际旧snapshot，包含热配置后旧binding触发的动作。没有改公共Engine/new/activate/handle或JSON/schema接口，也没有绕过UnsupportedActionTiming。

## 下一阶段主动作计划

RuleSelection.plan_main仅生成主to计划：Simple→Complex，Complex→Output；使用保存的eligible动作和旧snapshot，device/source/start/batch由原actor调用方提供。repeat/lazy/halt/scoped modifiers/hold_down_ns仍交给ActionPlan；保存的input-up release不能被替代为重新匹配up。

ActionPlan增加私有plan_action_refs，原公开slice wrapper与RuleSelection私有稳定索引迭代器共享同一preflight/展开实现。容量、scope、duration和时间溢出先检查，再创建输出计划；没有为了生成计划复制整段Action/conditions数组，只有正常计划里的target/事件复制。内部迭代器必须来自该immutable索引或slice；没有公开允许任意不稳定iterator跳过preflight的入口。

计划成功不等于入队/执行；下一步actor仍必须原子预留owner/source、保存batch/slot绑定、处理physical up、取消/device retirement、held/delayed分支和halt状态，并按ADR-0103跨批次保跨度。此接口尚未在原生runtime中执行duration计划，TerminalActor仍只执行Output stage。

## 验证

Rust1.97.1 locked/offline Linux workspace/all-targets973项测试、Clippy -D warnings、fmt和固定来源60项校验通过。新增7项tests：指定stage/first-match；Unknown规则跳过与filtered空规则不fallback；Shift具体侧别保存和额外modifier策略；条件计数只评估一次/旧Arc在配置替换后仍保留；实际Engine主数组先过滤、前序set_variable不能让后续条件动作重新生效；Simple/Complex生成正确next stage且保旧snapshot/device/source/50持续时间/input-up release；计划overflow无执行且可用相同batch重试。

原条件、变量、modifier、lazy/halt、held/alone/delayed、热配置/多设备/取消/阶段duration回归与现有无输入harness保持通过。纯Rust，无新unsafe/FFI/依赖；native ABI未变未重跑oracle。没有读取真实键盘/设备、原生duration配置、driver或安装。Windows该共享重构及前序native验证仍待补，不从Linux/mock结果推断Windows可用。见 [验证记录](validation/linux-2026-10-11-rule-selection.json)。

下一步将选择结果/下一阶段计划接入完整deferred actor的slot与source生命期，交付simple/complex计划及原始up，迁移fresh held/alone/delayed/halt/变量语义与资源预算后，才能解除原生duration gate。simultaneous、Windowsreload/验证、用户控制、图形消费者/hotplug/LED/UI/发布/driver等原目标不变。
