# ADR-0159：旧快照的有序组合候选仲裁

仲裁器保存首个 Complex Down 的原 Arc；每次 advance 使用同一当前可信条件/修饰视图，按原配置顺序检查尚未提交的候选，遇到 eligible 普通规则后不枚举后续规则。main 动作条件只在最终选中时过滤一次，保留 matched-empty 消费行为，不执行输出或变量。最终 RuleSelection 冻结选中时的修饰/消费集合；已提交选择不再匹配。

这是对 ADR-0158 接入清单中“首次输入冻结条件/修饰视图”的修正：上游 `basic/basic.hpp:203–260` 每次等待后的 manipulate 都在 lookahead 前检查当前条件和 modifier manager；候选尚未成为绑定，不能过早固定 eligibility 真值。旧配置 Arc 与已决定的实际绑定仍保留，fresh view 的改变可终结尚在等待的候选并进入后续规则。

同一时刻只运行一个 ChordWindow。较早组合尚在等待时，较后已能匹配的组合/普通规则不能抢先；较早失败后用返回的原 owned receipts 尝试下一候选，cursor 单调前进，不能重新进入刚失败的规则。不为每个候选复制输入缓冲，输入总界限仍为 256。

每次 advance 默认最多 64 个新规则推进/原 receipt 步，显式 budget 只允许 1..64；已有窗口的当前 eligibility 重检属于下一 receipt 步，避免 budget=1 永久饥饿。用尽返回 Yield，原窗口、cursor、tail 与旧 Arc 留在唯一 owner，下一次有界推进继续。Yield 不是等待 deadline 已确认或虚假的匹配/普通输出。单个规则条件及最终动作过滤仍按已有有界 IR 执行，不承诺硬墙钟时延。

窗口失败后原缓冲与未喂入 tail 重新按原顺序合并；匹配、普通映射或 pass 返回全部原 inputs/tail 和当前仲裁生成时刻。earliest/delivered 历史不被改写。后续实际 actor 重放时必须以当前 stage clock 准入原事件，不能用旧 delivery 生成已经过期的 Down/Up 或伪造 OS 时间。

候选 deadline/frontier、边界 incoming 保留、强制容量/来源错误都沿 ADR-0158。规则选择使用 private RuleHeader→RuleSelection，外部不能替换旧 index/Arc 或消费集合。仲裁不证明参与者绑定、共享输出、捕获或 native 接受；KW-SIM-001/UnsupportedSimultaneous 保留，后续必须完成共享组、条件身份、scope/lease、回放和 repeat 接入。
