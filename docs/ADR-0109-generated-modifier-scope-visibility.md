# ADR-0109：生成时修饰键作用域与原 Up 凭据

## 决定

GeneratedStageQueue 新增 enqueue_scoped，借用 ADR-0107 的 ScopedActionPlan，同时处理冻结动作与生成时作用域。该 plan 仍由调用方保留给实际执行 actor；生成队列不会代替 TerminalActor 开关真正的逻辑输出 owner，也不提供 native 权限。

目的队列的 from modifier 匹配必须使用带作用域的生成视图。main/Held 的最后合格动作是 modifier 时，captured consumed owner 从后续匹配视图隐藏，直到原输入 Up **生成入队**；最后动作是非 modifier 时，整个初始数组生成完即恢复视图。snapshot tap 分支临时范围生成后恢复原视图，保留其已生成变量 Down/Up 值。这里的恢复时刻与实际输出 actor 在延迟事件执行后的恢复时刻分离，poll 不重放这些逻辑变更。

逻辑作用域保存 consumed ActionSlot 引用，叠加隐藏计数，包含此前已隐藏的 owner。交错作用域以任一顺序结束都只解除自己的引用；原 owner 先生成 Up 时，旧引用仍保留到作用域结束。禁止原槽在仍被隐藏引用或所属 scoped 批次仍活动时重新 Down，防止旧引用误隐藏新 owner。完整 actor 仍须保证所有 batch/slot 跨生命周期不复用。

## 原子准入和资源预算

阶段及当前 Arc 校验包括空计划；原事件 prepare overlay 与原时间线准入成功后才提交逻辑作用域。拒绝时原 plan 仍属于调用方，source/设备/config/时间/条件结果不需要重新生成。未提交变量、modifier、隐藏引用和 scoped recipe 都不改变。

逻辑 source/owner 峰值检查包含 snapshot 临时 modifier 数量，live slots 总上限16384。隐藏引用按当前持有引用加本次 captured consumed、snapshot 之外已有 modifier 及 snapshot consumed 临时引用的瞬时需要检查，最大16384。当前逻辑视图只需持久保存最后 modifier 的 consumed 引用，其余临时引用在同次生成完成后归还。最多4096个等待原 Up 的 scoped recipe；空计划和没有原 Up 的已生成 tap 数组不留下 phantom recipe。

这种预算不代替实际执行 actor 的预算：多个在逻辑生成中已完成的 tap 仍可能在实际输出时间上交错，实际 actor 仍要独立检查和恢复。

## ScopedGeneration

enqueue_scoped 返回不可 Clone 的 ScopedGeneration，保存该队列的私有 Arc 身份、原 ActionPlan Up recipe 和一次性 up_generated 状态。它不暴露身份构造接口，不是 native session/activation/lease 代号。

input_up_scoped 验证队列身份、时间不倒退、原 scoped recipe 尚未取消，然后 prepare 保存的旧 Up。时间线成功才提交 Up 的生成时变量/修饰键更新、关闭逻辑作用域、推进原队列时间并锁存一次生成；失败保留原 recipe 和状态，可在容量恢复后重试。重复已完成 Up 返回 false，但仍先检查身份和时间。已准入旧 Arc 的 Up 可以跨 activate 使用，不能改为当前规则或新条件。

最后 modifier `repeat=false` 的 Down/Up 都已生成，且 ActionPlan.releases 可能为空，仍须等待这项原 Up recipe 才恢复 consumed。因此空原 Up 数组也是有效逻辑恢复，不会虚构 key 事件。实际 TerminalActor 的 ScopeUp 事件及持续时间约束仍由其独立执行路径维护。

取消、设备移除/代际替换清理对应逻辑作用域，旧未完成凭据再用返回 Lifecycle。reset 清逻辑作用域和引用并更换本地队列身份，全部旧凭据返回 Provenance；它不续租或更换任何 native run/epoch。设备/取消返回的生成 slots/events 仍不是完整实际输出 owner 清单。

## 验证

新增10个集成测试：原 Up 生成先于未来输出时恢复匹配视图；两重隐藏按任一 Up 顺序解开；snapshot tap 保留变量 Up 值并恢复 modifier 视图；scoped时间线溢出零变量/隐藏/队列变更、原 ID 可重试；设备/reset清理与凭据失效；last tapped modifier没有逻辑Down仍等原Up；精确16384引用拒绝不改状态且释放一个凭据后可重试；其他队列不能使用凭据、重复batch及空计划stage/Arc校验；旧ArcUp在8192满队列时保留scope/时间，腾一事件后重试及倒退时间拒绝；原owner先逻辑释放后仍被scope引用的槽不能重绑定。

Linux 工作区1011个测试用例通过，Clippy -D warnings、fmt、上游60个源码hash校验通过。没有新增 unsafe、FFI、依赖或 ABI，未重复 ABI oracle。本轮 focused及完整检查没有测试失败。Windows 原生补验仍待；共享 Linux 结果不作为 Windows 证据。无真实键盘、设备、安装或驱动操作。

## 未完成的连接

目前作用域参与当前目的队列的生成匹配视图；**尚未把修饰键作用域的差分事件传递给再下一目的队列**。完整阶段 actor 必须在 Simple→Complex→Output 的执行/生成边界传播这些效果，并把保存父子绑定与源码来源预算连接起来。当前组件不应直接替换完整 stage pipeline，raw事件的 poll 也不会携带实际作用域恢复权限。

context 更新事件、held/alone/delayed 协调、保留旧父规则生成新孩子的准入、source子树预算、native duration、simultaneous、Windows adapter、硬件与UI/包装继续。生产 Engine 的 UnsupportedActionTiming gate 保留；本 ADR 不声称完成原生 duration 或完整移植。

[验证记录](validation/linux-2026-10-11-generated-scopes.json)。
