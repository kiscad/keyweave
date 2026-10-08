# ADR-0017：末端阶段执行与保存的 Bound arena

状态：末端 actor 已实现并离线验证；多阶段规则匹配 actor/CLI 接入未完成。2026-10-08。

## 实现

TerminalActor 接收 action_plan 的 Output 阶段计划，推进 StageTimeline，并执行实际键/变量动作。键 down 调用已有引擎的最终阶段 owner/modifier 逻辑；保存真实 Bound 到 batch+slot arena。up 取出同一个 Bound 完成释放，不重新匹配或创建新 owner。计划器的事件数组改为只读访问，防止调用者修改已预检的生命周期；只有 crate 内执行器可移动事件。

actor 私有 core 允许 action duration IR，但只调用 press_stage 的最终阶段。普通 Engine::new/activate 的 duration gate 保留，TerminalActor 对 Simple/Complex 计划明确 UnsupportedStage；这是阶段执行组件，不是另一套缩水配置引擎。计划需来自 actor 当前 Arc 快照；已接收批次和已安排 up 在更新配置后保留旧快照。新 cfg 预留变量名字的过程与原引擎一致，旧变量 up 不丢失。

批次接收前预留全部 slots 和 SourceToken range，失败不推进 source/batch 身份。最多 4096 live batches、16384 reserved slots，时间队列继续 8192/单批 1024。不同批次同键拥有独立 source，先释放一个不影响另一个。distinct planned/emitted keys 保守限制为 512，使设备/批次撤销时 pending modifier 的最多一个 down 与一个 up 仍在 1024 输出预算内。已完成批次留下的 latent emitted modifier 也计入此预算。

input_up 把保存的释放命令安排到不早于计划末尾的时间，重复/已完成 up 不创建额外释放。若有已到期队列项，先返回 DeadlinePending，调用者先 poll 到期工作再处理普通物理事件。同刻 Reset/remove 是控制优先；它们撤销未来工作、只清理已执行 owner。observe_device 维护原代际历史；新代际结束旧批次，其他设备保持。未知批次取消/无关连接不刷新 lazy 状态。Reset 清 slots、队列与 reservation，保留身份历史与 source/batch 单调性。

poll 每次执行一个工作项，迟到用实际交付时间输出。halt_requested 返回给上层原 rule binding；当前已安排数组继续。末端 actor 不自行把 batch 当作 halt scope，也不声称处理 alone/held/delayed 的原规则生命周期。

## 边界与后续接入

本组件输出仍是 intent。输出队列通过 known-prefix acknowledgment 独立维护 submitted；actor arena 清理不替代平台恢复。联合测试验证 timed up 已在 intent 中释放，但 backend 拒绝 up 时 submitted 仍保留 down 并另行清理。

新批次只能使用当前快照；后续多阶段 actor 的旧 binding 子批次（alone/delayed 等）需要受控扩展旧快照接收与共享名字/预算，不能让任意旧配置绕过激活。上层还需整批 action 条件筛选、从 root 保存 mandatory/down modifiers、实际阶段规则匹配、可撤销宏、物理 Repeat/bypass、context 的同刻顺序及 config generation 切换。当前 CLI 不使用 TerminalActor，duration 配置字段仍拒绝。

## 验证

13 个 actor 测试：真实定时 down/up、早输入 up、scope/lazy、同键多来源、拔除/未执行 down 撤销、Reset、持键更新/变量 up、迟到/halt、无关控制、非末端/外来快照拒绝、16384 slots 预留与容量回收、8192 队列容量以及 latent emitted key 预算。另 1 个 actor→OutputQueue 部分提交联合测试。无 OS、硬件或 Windows 验证，未修改上游 oracle。
