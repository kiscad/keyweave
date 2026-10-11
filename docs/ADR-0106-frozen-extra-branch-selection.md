# ADR-0106：额外动作分支的生成时选择与冻结

## 实现决定

`RuleSelection::select_branch` 新增 Held、Alone、AfterKeyUp、DelayedInvoked、DelayedCanceled 五类选择，返回拥有旧 Arc 配置的 `BranchSelection`。每次调用在该分支实际生成时重新检查其 action conditions，只有 Truth::True 合格；不复用初始 main-to 的过滤结果，也不重新选择 mapping。所有条件先整体过滤，随后才可能执行第一个动作；分支内变量写入不能重新启用或禁用同一批次的后续动作。

BranchSelection 保存私有分支下标、旧 stage/index、原按下修饰键集合和实际 consumed 侧别。即使原 RuleSelection 被释放或新配置已激活，actions 和 plan 仍指向原不可变数组，不复制整份 Action/conditions。缺少 delayed 配置返回 None；已有分支全部被过滤或 halted 返回 Some(empty)，二者不能混用。调用方显式传入原 halted 状态，halted 不再评估 action conditions。

plan 调用原共用 action planner，保持每项 lazy/halt/repeat/modifier/variable/duration 选项和设备/source/旧 Arc 元数据。Held 使用 main 的最后可重复动作原输入 Up 绑定；Alone、AfterKeyUp、两个 Delayed 分支使用 tap_all，包含其自有变量 Up 生命周期及持续时间。Held 最后可重复动作的 hold_down_ns 不增加其 Up 的最早时间；其 Up 由原输入控制。上游 event_sender.hpp 的 post_events_at_key_down 中也只在非末项或 !repeat 时加入 hold_down_milliseconds；既有 action planner 测试已有此行为。

`BranchModifierPolicy` 保留执行所需的范围策略：Held/AfterKeyUp 使用当前修饰键并隐藏 consumed；Alone/两个 Delayed 使用原按下快照并隐藏 consumed。这只是策略与集合元数据，plan **不生成范围恢复或隐藏操作**。调用方必须保留 BranchSelection，并由 actor 在对应生成/执行边界维护实际 owner 和作用域；不能直接把 plan 当作已经处理修饰键的完整分支。

## 当前生产路径接入

共享 action_matches/filter_actions 供 main selection、branch selection 和递归 Engine 使用。Engine.press_actions 的 held/alone/after/delayed 条件过滤使用同一 helper；replace_held 先清理旧孩子，再一次过滤整份 held 数组，last-is-modifier 和实际 press_filtered_actions 复用同一结果，消除原 reverse-find 与再次过滤的重复条件求值。既有 halted、来源分配、清理及修饰键范围行为保持；整个已开始数组仍不会因为其中一项 halt 而中断。

检查变量热重载路径后确认：activate 保留变量名并检查历史名称与新配置名称的有界并集；reset 才回收非当前名称。因此终端 actor 的旧排队变量和旧 Up 所需名称不会在 activate 时消失。它依然是共享变量状态，不是按配置分离的变量空间；已赋值历史名称在 reset 前保留。此轮没有修改变量保留语义。

## 验证与边界

新增 6 个集成测试，覆盖所有分支的 fresh filtering/旧 Arc 生命周期、held 原输入 Up 与其余 tap 持续时间、Unknown/halt/缺失 delayed、Simple→Complex 和 Complex→Output 阶段、实际 consumed 侧别，以及真实 Engine main 设置变量后 held 全数组先过滤、分支内改变量仍保持已选 modifier 的 Down/Up 和 owner 清零。

Linux 工作区 982 个用例通过，Clippy -D warnings、fmt、上游 60 个源文件 hash 校验通过。第一次测试中 held Up 错误期待最短 50ns 保持；对照上游及既有 planner 后将测试改为原输入 Up 时间，未更改 planner。第一次 Clippy 指出可省略生命周期，已修正。最终记录见 validation/linux-2026-10-11-branch-selection.json。

无新增 unsafe/FFI/依赖/ABI，未重复 ABI oracle。Windows 原生编译及前序补验仍待，Linux 结果不代替 Windows 证据。本轮仅纯状态和既有无输入子进程检查，没有真实键盘、设备、安装或驱动操作。

## 下一步

select_branch 不决定 timer 是否有效、alone 是否仍成立、delayed 是否取消，以及原 halted 的更新时间。完整 stage actor 必须协调父子绑定完成、Held 替换、after-up 分支、作用域恢复、source/清理预算与队列原子准入。当前 TerminalActor 仍只执行 Output；生产 Engine 的 UnsupportedActionTiming 拒绝仍保留。simultaneous、原生 duration、Windows adapter、硬件/UI/包装及完整原目标继续。
