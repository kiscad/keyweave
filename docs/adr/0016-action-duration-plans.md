# ADR-0016：动作批次的 duration 计划生成

状态：纯计划器与队列联合验证已实现，actor/配置执行接入未完成。2026-10-08。

## 选择

新增 engine/action_plan.rs。Action IR 增加 hold_down_ns，默认 0；配置编译仍拒绝 hold_down_milliseconds/held_down_milliseconds，不把未实现功能偷偷忽略。当前递归 Engine 构造/activate 遇到任何非零 duration 会返回 UnsupportedActionTiming，旧快照保持有效。actor 接入后再移除这一明确的临时 gate。

plan_actions 接收生成阶段已整批条件筛选的动作、原配置/设备/source、目的 stage、开始时间以及 tap_all。它生成带 down/up、lazy/halt、保存 binding slot 的 StageEvent；末端 owner 和实际输出尚不在此发生。普通数组的前面动作、repeat=false 末动作、所有 extra 动作均完整轻触，main up 前累加 duration，下一动作从累加后时间开始。普通末 repeat=true 保持到原输入 up，duration 不让它自动松开；输入 up 命令不早于整批计划末时间。

modifier scopes 在 main down 前进入下一阶段；普通非 modifier main 使用 lazy scope down，modifier main 受 to.lazy/tap_all 控制。立即 scope up 等 main tap up；普通末 modifier 的 scope 保持至原输入 up，即使 main repeat=false。已保存的输入 up 顺序保持现有引擎的 main 先释放、scopes 逆序释放；普通立即 scopes 维持原数组顺序。目标 variable 的 typed down/up operation 保留，actor 的 up 分支只完成已安排的 key_up_value，不重新赋 down 值。

每个批次有 actor 分配的 BatchId，slot 是 batch+index；同一原物理 source 的不同 to/alone/held/delayed 批次不能串用 slot。down/up 用同一个 slot，next-stage 重映射后的 up 必须释放保存的 Bound，不能重新匹配。BatchId 不是配置 generation，也不是新 output owner；actor 必须保证唯一、checked overflow 和有界 live slot arena。

预检最多 512 slots/1024 计划边沿（含输入 up 预留）、每动作 duration 至多一天、累加时间 checked overflow、scope 仅标准 modifier 且主目标是 Key。先预检再构造，失败没有可执行的部分计划。计划器不依赖系统时间、OS 或 mutable configuration。

## 接入约束

阶段执行器仍待实现；计划器与 StageTimeline 已有联合测试，但不能称 CLI replay 已支持 duration。actor 需要 batch→原 rule binding 的关联，使 main halt 改原 binding 状态，当前数组仍全部执行，后续批次跳过；不能仅以 BatchId 作为 halt scope。生成阶段整批筛选与下一阶段实际交付时重新求规则条件是两件事，不能规划未来时提前更新 variables/owners。

actor 接入还需处理 owner/source cursor 预算、mandatory suppress/restore、held 替换、alone 快照、延迟 pending、Reset/remove 的已执行与未执行工作区分。取消计划需解除 live slot 与预留，已执行 down 仍要完成实际 up；不能从撤销的计划凭空制造输出。原始来源与配置通过每个 StageEvent 保留。

## 验证

12 个新增测试覆盖累加 duration、最后持有、repeat=false、extra taps、modifier scopes、lazy/halt 当前数组、变量 up、slot 不串批次、队列按时推进/旧配置保留、空计划、容量/时长/溢出/非法 scope 和递归引擎明确拒绝。时间使用人工纳秒；参考语义来源同 ADR-0014 的已固定源文件，没有转换 Mach 单步自增或更改上游 oracle。尚无 Windows/硬件验证。
