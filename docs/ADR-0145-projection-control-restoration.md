# ADR-0145：作用域生命周期取消的即时恢复控制

作用域触发设备移除会丢弃未来 close marker；被隐藏的来源可能属于另一设备，不能随 trigger 一起删掉来源，也不能依赖普通 close 排队（队列可能满，且生命周期控制必须先于 due）。

Output 提供 restore_key_projection_control：预检两侧投影的身份、引用预算和时间后，删除该投影待交付控制，解除原 mask，恢复仍存活的变量原 Down 和普通键可见性；modifier 保持原 lazy 同步策略。未激活的投影只撤未来隐藏，出生投影在批次首动作前取消时还须清 birth 标志。原动作数组/slots/Up/其他投影保持，旧来源不重建，无条件重选或 macro replay。已排 close 的生成侧已经恢复，控制不再重复赋变量 Down。

Complex 提供对应父模板引用控制：只移除指定引用，末引用对其保存 Output 投影即时恢复；不要求 drain due，不分配队列空间。组内任一异常仍 Failed/Reset。所有返回 intents 必须交给 caller，不能丢弃。纯 Rust 控制不是 OS 输出接受证明。

该接口供后续 Simple scope 的跨设备生命周期清理使用；本 ADR 未撤 mandatory/native gate。

原 Up 会清对应 owner 的 hidden 条目，凭据仍可保留；预检只对仍有 owner 的来源要求 mask。两侧缺失/不一致属于内部失败；未知 token 是冷拒绝。多个孩子控制不是整组撤销事务，发生异常时不能宣称先前计算的 intents 已被 OS 接受，恢复必须依据独立接受账本。
