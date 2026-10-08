# ADR-0004：轻触与松开动作保存阶段绑定和预算

状态：采用并实现纯键动作；2026-10-08。

问题：`to_if_alone`、`to_after_key_up` 依赖 down 时选择的规则；热更新后查新快照会错发动作，拔设备时发宏会产生用户未请求的输入。仅保存最终键列表也无法结束中间 tap 派生规则的额外 up 动作。

选择：每个物理 binding 保存不可变旧快照、阶段规则索引与子 binding 树。正常 up 先结束子输出，然后按严格 `< timeout` 执行 alone，最后执行 after-up；额外动作始终完整轻触。阶段中间动作的 up 同样结束其自己的派生规则，之后才处理下一动作。拔设备、Reset 不执行额外动作。

alone 候选只属于最近一次成功的新物理 down；新 key/button down 与非零 wheel 取消它，move、零 wheel、重复、重复 down 与 injected bypass 不取消。当前 wheel/move 只提供离线事件和 Pass 决策，没有捕获后端或指针映射实现。物理 down 失败不改变已有 alone/ownership 状态。

修饰恢复：保存 down 时可见的八个 HID 左右修饰键。alone 轻触作用域暂时屏蔽当前多出的 modifier owners，并为缺少的历史修饰键创建临时 owner；完成后撤销作用域。逻辑 owner、临时 suppression 与已发出的 pressed set 分开保存，避免释放别人的输入来源。Fn、Caps Lock 锁状态尚未纳入，不能宣称完整复制上游 modifier flag manager。

预算：激活时计算并缓存每个输入键所有可能阶段展开的上界，包含 to、alone、after-up 和至多八个临时修饰生命周期。down 为完整 binding 预留 SourceToken 区间与 owner 预算，后续旧快照动作使用原区间。通过保守预算使正常 up 不因热更新后输出预算被占用而失败；释放预算在 binding 结束/拔除/Reset 时回收，token 不重复使用。限额仍为 512 生命周期、16384 全局预算与 4096 bindings；实际预留比同时 held owners 更保守。

参数：默认 alone 为 1000ms，允许 global 与 manipulator override。四个上游 basic timing 参数接受整数 0..=86400000ms 并保留；当前仅 alone 有执行行为。held/delayed/simultaneous 必要规则字段仍拒绝，所以单独保存这些已知参数不暗示那些特性已实现。上限为 KeyWeave 资源约束，不冒充上游无限上限。

证据：固定上游 basic.hpp 的 key_up 顺序、严格超时检查、unset_alone_if_needed；event_sender.hpp 的 scoped_from_key_modifier_flags_state_restorer / post_extra_to_events。tests/tap_lifecycle.rs 与 fixtures/tap-alone.json 固定阈值、取消、旧快照、修饰恢复及故障清理。未转换上游 Mach 时间，也未宣称已完成所有高级规则。
