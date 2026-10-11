# ADR-0110：保存绑定树的查询与整棵子树取消

## 背景与实现

上游 event_sender.hpp 的 post_from_mandatory_modifiers_key_up/down 通过 lazy modifier 事件传递隐藏与恢复；下游可能已对这些 modifier 建立重映射绑定。完整阶段传播需要能够识别保存的下游节点、未来预留节点和原配置来源，不能只根据终端当前 modifier 集合生成无来源的 Up/Down。

BindingArena 的每槽 child 计数改为 BTreeSet<BatchId> 邻接索引。只有 Live 父槽可以接孩子，沿用同设备/source/Arc 和严格 Simple→Complex→Output 的准入，不弱化阶段或生命周期约束。每个孩子仍只有一个父槽，图深度最多三层，不能形成环。prepare 的独占 Reservation 在 commit 时才添加边；放弃预留不会留下幽灵孩子。

visit_subtree 要求根槽为 Live，以只读 BindingNode 回调给出 slot、原 PlanOrigin、parent 和 BindingState。孩子批次全部槽可见，状态包括 Reserved、Live(&T)、Released；回调可区分未来工作和实际保存的 payload。根 Reserved/Released/不存在/越界在任何回调前拒绝。遍历当前槽的孩子，不包含同批次其他根槽的兄弟子树；不 Clone payload，不变更生命周期、不分配来源、不执行平台操作。

该查询是跨阶段作用域实现的基础，不等于已经完成传播。特别是 modifier 映射到普通键或变量动作时，后续 actor 必须处理保存绑定和变量 Up 的语义；不能把它们都当作 modifier owner 或凭查询自动发送键盘事件。尚未执行的孩子也需继承作用域意图，不能为 Reserved 槽虚构清理 Up。

## 取消

cancel_subtree 先只读收集全部 descendant 批次的取消顺序，成功后在同一独占借用下逐项拆树。顺序为孩子先于父；同一父批次按槽号倒序，同槽孩子按 BatchId 倒序；每个批次的 Live payload 也按槽号倒序返回。这个次序确定且符合父子依赖，通用 arena 不额外假定 BatchId 等于物理时间。

每项 CanceledBatch 拥有原 PlanOrigin、parent 和 CanceledSlot(slot,T)。配置 Arc 按批次保留，不重新选择或克隆整个配置。即使只有 Reserved/Released 槽且无 Live payload，也返回该批次的记录，供调用方丢弃未来队列工作。已释放或未执行槽不出现在 payload 清理列表；移除孩子同时拆除父邻接边，单独取消一个孩子子树不会影响其他兄弟。

拆树不调用任何输出恢复或额外动作。调用方必须先丢弃所有返回 batch 的队列事件，再执行 payload 清理并恢复普通运行；记录中不包含完整平台 ledger。Drop 不是恢复证明，清理失败仍须原 runtime 的恢复机制处理。

既有单批 cancel 和 retire 使用相同 remove_batch 路径，继续在孩子存在时拒绝。整棵子树取消不存在根在 mutation 前拒绝。私有图仅能由已校验的准入/退休方法修改，所以预检后的 postorder 拆树在独占借用下不会出现孩子未拆或父提前消失；内部 impossible 状态仍视为程序不变量违背。

## 终端 actor 接入与资源

TerminalActor.cancel 接入 cancel_subtree 的 owned payload/原 Arc 清理路径。它目前全部批次都是 Output 根，没有孩子，因此旧取消的输出顺序、source cursor、extra_actions=false、scope 清理和变量 Up 行为保持。当前只丢根 ID 的时间线事件在这个不变量下覆盖全部记录；未来完整阶段 actor 必须丢全部返回批次，不能照抄这个单阶段假设。

批次上限4096、预留槽上限16384不变。邻接索引让查询/收集取消沿真实边进行，避免逐节点扫描整个批次表；最多访问当前子树的有限槽和边，递归深度受三阶段约束。BTreeSet 元数据比原计数占用更多内存；这不是硬实时、固定 RSS 或 allocator 成功保证。

## 验证与后续

新增6测试：三阶段树查询保旧来源且区分所有生命周期状态/不要求T Clone；三层与兄弟树 postorder 取消、仅Live payload、旧Arc在清理记录释放前存活；无效根无回调/无mutation；单孩子子树拆边、兄弟保留与父可释放；放弃 child reservation 不留下边；4096批次最大树查询与取消，4095个未执行子批次仍返回丢队列记录，但只返回一个真实根payload。现有 TerminalActor 取消、设备移除、scope、reset、变量与source共享测试继续通过。

Linux 工作区1017个测试用例通过，Clippy -D warnings、fmt、上游60个源文件hash校验通过。无新增 unsafe/FFI/依赖/ABI，未重复 ABI oracle。Windows共享/native补验仍待，Linux结果不代替Windows证据。本轮没有真实键盘/设备/驱动/安装操作。

跨阶段作用域的lazy差分传播和完整 Simple/Complex 执行仍未完成；接下来须结合该树、GeneratedStageQueue、ScopedGeneration 和来源预算处理 modifier/普通键/变量孩子、未来预留孩子继承及恢复边界。context更新、held/alone/delayed、旧父生成新孩子准入、simultaneous、native duration、Windows adapter、硬件/UI/包装及原完整目标继续。UnsupportedActionTiming 生产拒绝仍保留。

[验证记录](validation/linux-2026-10-11-binding-trees.json)。
