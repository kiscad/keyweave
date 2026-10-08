# ADR-0013：显式 lazy 与 binding halt

状态：已实现并离线验证；2026-10-08。

lazy 是动作布尔选项。标准 modifier 主目标的 down/up 只改逻辑 owners，等下一普通 down、显式 modifier 事件、指针允许刷新或 device neutral 再输出；非 modifier 的键事件仍正常发出。主 modifier 的 to.modifiers 在 lazy=true 时也用 lazy down；extra taps 的 scopes 本来就是 lazy。下一阶段若重映射该事件，其动作定义决定新的 lazy 策略。

halt 是同一 rule binding 的持久布尔状态。当前过滤后的动作数组继续全部执行；已安排的主输出 up/变量 key_up_value 始终完成。后续 held、alone、after-up、delayed 批次不再执行新动作。alone/held/delayed 中 halt 可以阻止之后的批次。状态由 Bound 和 DelayedPayload 共享 Arc<AtomicBool>，使用 Relaxed 且只由引擎 actor 写；使用 Atomic 仅保持 Engine 可移动到执行线程，不允许多 actor 并发访问引擎。配置快照没有此 mutable 状态。

非键变量动作 down 与上游非 modifier key_down 一致，会刷新当前 logical modifier 状态；其 lazy 标志不推迟变量赋值。变量 up 值和普通 owner release 不受 halt 阻止。过滤掉的 halt 动作没有作用；热更新不改变原 binding 的状态，Reset/remove 正常清理。

输出边沿预算与 SourceToken/owner reservation 分开：当前配置缓存输出上界，包含最多八个潜在 modifier 同步边沿；简单阶段采用原展开上界*2+8，complex 结合正常/held 批次和已有 snapshot/scope 预留。容量不足在原 down 前拒绝，编译器也诊断超大展开。保留原 source range 与 4096 binding/16384 reservation 上限，避免把边沿同步误当成新的 owner。

Caps/Fn/lock/sticky 扩展仍有独立能力限制，hold_down_milliseconds/simultaneous 尚未实现。纯离线输出不证明 OS submitted 状态或真实按键延迟。
