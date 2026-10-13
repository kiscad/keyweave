# ADR-0154：Simple 保存父规则的 after-key-up 子批次

Simple 原 Up 已通过保存凭据校验并提交后，以原 ProducerDecision 的旧 RuleSelection 选择 AfterKeyUp。动作条件使用本阶段当前生成视图和本次可信 context；规则本身不重匹配。生成 child、外部父树预留和 Complex 输入登记复用同一联合 prepare/commit。数组保持 duration/FIFO，分支是 tap，默认不另等待物理 Up。

一个 Simple producer 可同时保留尚未完成的 main child 和 after-up child。bindings 仍只指向 main/repeat；分支退休不得清掉另一个孩子的关联。父树只有所有孩子与 consumed scope close 完成后才能退休。原 Up 后分支准入失败沿已有 Failed/reset 契约，不承诺整图回滚。

AfterKeyUp 使用已有 CurrentMinusConsumed scope，复用来源投影传递；不解除 Alone/Held/Delayed gate，不新增定时器或 native 入口。原来源预算只计 Simple main，本轮同步把 after-up 的每个 action/modifier 及其 Complex 所有分支扩张纳入物理根保守预留；本轮纯 actor 测试，不操作输入设备。
