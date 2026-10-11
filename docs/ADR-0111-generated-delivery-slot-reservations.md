# ADR-0111：生成状态之外的交付槽位预留

## 问题与修复

GeneratedStageQueue 的逻辑 Down 在生成 Up 时移除。对于未来的 tap，Down/Up 都已生成而尚未从时间线交付，旧实现的逻辑表已经为空，原 BatchId/ActionSlot 可以再次进入队列，形成实际绑定身份冲突。修复增加私有 DeliveryLedger，区分生成状态、事件交付状态和显式退休；逻辑表为空不能作为释放交付身份的依据。

每个批次保存统一的 device/source/旧配置 Arc 和独立槽表，槽表保原 ActionTarget、down_delivered、up_generated、up_delivered。初始 Down 必须属于新批次；同一次追加数组可以创建该批次的多个唯一槽，全部必须具有一致来源。活动批次只接受保存的 Up，不接受新槽扩展或重复 Down，即使其逻辑视图已经为空。Up 必须具有原完整来源和 target，重复生成也拒绝。

生成逻辑 overlay 与交付预留均先准备，原 StageTimeline 准入成功后二者才提交。任何逻辑、来源、生命周期、资源或时间线失败都不留下新预留。scoped 和 saved original Up 通过同一路径更新交付状态；已准入旧 Arc 的 Up 仍可跨 activate 追加，不重新选择配置。

poll 仅标记该事件已从生成队列取出，保持生成时变量/修饰键不重放。这些标记 **不是** native OS 接受，也不是下游绑定已经清理的证据；它们只是队列交付进度。

## 退休与控制

新增 reserved_slots、pending_batches、delivery_progress 查询；进度只提供数量，不输出输入轨迹、变量值或配置正文。reserved_slots 计算全部交付槽及 scoped snapshot 额外来源预留，已逻辑结束的 tap 和临时来源在显式退休前继续计入。

retire_batch 要求 scoped original-Up recipe 已生成并关闭，且该批次每个槽的 Down 已交付、Up 已生成、Up 已交付。部分槽完成、仍 held 或 ScopeUp 尚未生成时均拒绝。完成全部事件也不会自动退休，调用方须在其 **实际下游绑定和作用域** 完成后调用；账本不能代替 BindingArena、TerminalActor 或平台 ledger 验证这些事实。

该方法只释放此队列的预留。完整 actor 的 batch allocator 仍须保证身份跨生命周期不复用；取消或退休后的 ID 不应在仍有外部绑定/延迟任务时重用。当前 DeliveryLedger 不分配真实 source，也不改变任何 native epoch/lease。

cancel_batch、设备移除/代际替换、reset 同步移除相应交付预留；时间线先丢弃未来事件，不为未执行槽虚构 Up。原控制返回的 logical slots/events 接口保留，仍不是完整实际 owner 列表；调用方必须独立清理保存的实际绑定。reset 继续使旧 scoped 凭据失效。

## 边界

每个队列最多4096个未退休交付批次、16384个槽及 snapshot 来源预留。快照额外来源即使逻辑 scope 已在同次生成中关闭也保持预留到退休；这为实际延迟执行保留空间。队列仍最多8192事件/单次1024事件。这里是每队列预算，尚未形成完整输入子树跨所有阶段的统一 source pool。

账本只复制有界 action target 和 Arc 元数据，不复制整份配置；allocator/RSS/时间不具有硬保证。内部交付标记依赖私有准入与同阶段 FIFO：时间线中的每个事件都有账本，Up 取出前 Down 已取出，控制移除批次前同步丢弃其排队事件。

## 验证

新增8个集成测试：future tap 逻辑空仍占预留/拒绝别名/全部交付后仍等显式退休；同批来源冲突与初始Down扩展拒绝；scoped快照来源跨逻辑关闭仍预留；last tapped modifier全部KeyUp交付仍等原scope Up；cancel/device代际/reset回收；时间线溢出无幽灵预留、原ID可重试；旧ArcUp跨reload/空计划无批次；多槽部分Up不能提前退休整个批次。

既有16384逻辑槽压力测试改为每批8槽，以单独覆盖槽位上限且不先触及4096批次上限。8192满队列 scoped Up 重试测试改为4094个tap批次，其中一个两动作，使加上原2批次后正好4096批次、8192事件；容量断言与失败/重试语义保留。首次 focused run 的旧单槽单批压力布局触及新批次边界而失败，已按上述布局修正，未放宽限制。

Linux工作区1025个测试用例通过，Clippy -D warnings、fmt、上游60文件hash校验通过，无新增unsafe/FFI/依赖/ABI，未重复ABI oracle。Windows原生和前序共享改动补验仍待。仅纯状态/已有无输入进程验证，无真实键盘、设备、安装或驱动操作。

完整阶段 actor 仍需组合逻辑视图、交付预留与保存绑定，并在子绑定和实际scope结束后协调退休。scope差分传播、旧父生成新孩子准入、统一source子树预算、context/分支、simultaneous、native duration、Windows adapter、硬件/UI/包装及原目标继续。生产 UnsupportedActionTiming 拒绝保留。

[验证记录](validation/linux-2026-10-11-delivery-reservations.json)。
