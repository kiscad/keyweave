# ADR-0108：目标阶段队列的生成时逻辑环境

## 上游依据

`basic.hpp` 的 from modifier 匹配使用 output_event_queue 的 modifier_flag_manager，而非输入事件缓冲或已经提交到设备的状态。同文件的条件检查使用目标队列 manipulator_environment。`event_queue/queue.hpp::emplace_back_entry` 先保存事件，然后立即更新 modifier_flag_manager 和 set_variable/key_up_value；事件的未来时间戳不会推迟这些逻辑更新。这两个文件都在现有 60 个已校验源码清单内。

因此需要区分生成状态和实际输出状态。带持续时间的 modifier tap 可能已经把 Down 和未来 Up 都排入队列，此时规则匹配的逻辑状态已经是不按下，而实际终端输出尚未 Down，或者仍在等待 Up。变量的已生成 Up 值也不能在输出延迟到期时再次写回，覆盖后续生成的赋值。

## 实现

新增 `generated_stage::GeneratedStageQueue`，每个实例绑定一个目的 Stage，组合既有 StageTimeline 与独立 Environment。Complex 队列 select 检查 Simple mapping，Output 队列 select 检查 Complex mapping；它使用该目的队列的生成时 modifiers 和变量，并接受调用方的 context/device/生成时间进行三值条件判断。Simple 目的队列没有前置 mapping selector，select 返回 Stage 错误。实例间不共享修饰键或变量状态。

生成 Down 保存 ActionSlot、设备代际、原 source、旧配置 Arc 和原 ActionTarget。Up 必须与所存设备/source/Arc/target 全部一致，释放保存逻辑绑定；原规则不会重选。lazy 修饰键同样影响逻辑状态，与终端惰性输出分离。重复 Down、未生成 Down 的 Up、同一追加数组内 Down/Up 后复用同槽及 Repeat 都拒绝；批次 ID/槽跨生命周期不复用仍由完整 actor 负责。

变量在追加顺序中执行 Set/Unset 和保存的 Up 值；不执行数组内条件再过滤。调用方先用 RuleSelection/BranchSelection 冻结整个数组。变量名必须已冷预留，值必须有界，空 Set 操作被拒绝。提取共享 variable_truth helper 给 Engine 和生成环境使用，保留 missing=Integer0、严格值类型和 unless 语义。非变量条件仍由原 Condition.evaluate 的 context freshness/Unknown 规则判断。

prepare 以最多 1024 个追加事件构造变更 overlay，逐事件校验来源和 live slot 峰值，不复制整个历史表；不修改当前 modifiers/变量。随后 StageTimeline 准入成功才 commit。任何逻辑校验或原队列容量/device/order/time-overflow 失败都返回原 owned events，旧环境和队列不变。逻辑 live slots 上限为 MAX_OUTPUT_OWNERS=16384；队列仍是 8192 个事件、单次 1024 个追加事件。

poll 只交付到期 owned 事件，保持 delivered_at 和 FIFO 原规则，完全不重放变量赋值或逻辑 modifier 边缘。返回事件仍需执行 actor 完成真正绑定与输出，不能把 poll 或生成状态当作 OS 接受证据。

## 重载及控制

activate 冷验证新 IR、要求 generation 增加，检查历史/新增变量名的有界并集，成功才添加新名字并切换当前 Arc。已生成 Down 所保存的旧 Arc Up 仍可追加；新的 Down 要求当前 Arc。已有旧队列事件不被重写。将来旧保存规则生成新 child Down 的授权仍须完整 actor 的父子绑定路径提供，当前接口不放宽这条门槛。

设备控制先由原 StageTimeline 验证代际/设备历史，再丢对应排队事件并移除旧代际或移除设备的逻辑 Down。cancel_batch 同时返回该批次的 discarded events 和逻辑 held slots。移除时按反向槽顺序应用保存的 variable Up 值，不运行额外动作。此前已经生成且完成逻辑 Down/Up 的变量赋值不回滚。

返回的逻辑 slots **不是全部实际输出 owner 清单**：tap 的逻辑 Up 已生成时，其实际 Down 仍可能在设备上保持。完整 actor 必须独立通过 BindingArena/实际 device 绑定清理这些 owner，不能仅凭逻辑 slots 数归零声称恢复。reset 清原队列/逻辑 Down/变量值并回收非当前名字；原设备历史仍由时间线保留。不存在 native 捕获、租约或恢复证明。

## 验证与尚未接入的部分

新增 7 个集成测试：lazy 未来 modifier Down 在 delivery 前参与 Simple 匹配且不同目的队列独立；timed tap/variable Up 在生成时改变视图、delivery 不重放；旧 Arc Up 跨 activate 及错误 device/source/Arc/stage 原子拒绝；时间线 overflow 返回原 events 并丢全部逻辑预备变化/同 ID 可重试；同数组复用槽失败不提交前序赋值；设备清理保存变量 Up/其他设备保留/reset 回收；16384 live slots 峰值预算拒绝下一变量 tap而不修改变量。

Linux 工作区 1001 个测试用例通过，Clippy -D warnings、fmt、上游 60 个源码 hash 校验通过。首次测试编译发现 StageEvent 不支持整体 PartialEq，改为逐字段及 Arc 指针身份验证原事件；首次 Clippy 提示冗余 pattern，已修正后重跑。无新增 unsafe、FFI、依赖或 ABI，未重复 ABI oracle。Windows 原生编译及前序平台补验仍待。

这个组件已在共享 engine 库实现并用真实 RuleSelection、ActionPlan、StageTimeline 组合验证，尚未接入完整阶段执行 actor 或 native runtime。它尚不处理 ScopedActionPlan 的逻辑作用域、context 更新事件或设备 runtime，也不负责原输入 Up recipe、父子绑定、source 子树预算、held/alone/delayed 协调。不要把 TerminalActor 的物理作用域状态直接接作本视图；后续须把 scope 的生成时效果写入相应目的队列。生产 Engine 的 UnsupportedActionTiming gate 保留；simultaneous、Windows adapter、真实硬件、UI/包装及完整原目标继续。

验证记录：[linux-2026-10-11-generated-stage.json](validation/linux-2026-10-11-generated-stage.json)。
