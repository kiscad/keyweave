# ADR-0107：延迟终端工作的修饰键作用域

## 实现与边界

新增 `modifier_scope::ScopedActionPlan`。main/branch 构造器同时从不可变 RuleSelection/BranchSelection 创建 ActionPlan 和私有 ModifierScope，二者不能被调用方换配。构造器验证 snapshot/consumed 集合仅包含标准修饰键且 consumed 是 snapshot 子集。动作过滤和时间规划继续复用既有实现；空计划不创建作用域或等待 Up 的幽灵批次。该包装本身不代表操作系统捕获或输出权限。

TerminalActor.enqueue_scoped 仍只接受 Output stage，且准入时要求原 Arc 是当前配置；已准入旧批次在 activate 后继续使用原 Arc。冷准入预留每个 snapshot 修饰键所需的 owner/source 区间、动作槽和不同输出键预算。预留与原时间线一同提交，时间线失败丢弃 guard，source cursor/batch ID/预留不推进；新的作用域直到首个排队动作真正执行才建立。

CurrentMinusConsumed 捕获当时 consumed 键的全部实际 owner；DownSnapshotMinusConsumed 捕获 snapshot 之外的修饰键 owner，并为 snapshot 中每个键建立独立临时 owner，再隐藏 consumed。这些 owner 只是逻辑状态，最终输出继续由原 sync_modifiers/key 生成。延迟作用域持有完整 snapshot，而非只补当时缺少的键：原物理 owner 或其他作用域 owner 可能先被释放，快照仍须保持到本分支结束。

每个作用域独立保存隐藏引用；包括已被别的作用域隐藏的 owner，使用计数叠加。任一作用域先完成或先取消都只解除自己的引用，不提前解除其他作用域。owner 提前释放时，其对应隐藏表项可删除，但仍保存的引用预算直到所属作用域关闭才回收。现有递归 Engine 的隐藏路径未更改；这套引用管理由 TerminalActor 的新作用域执行路径使用。

作用域建立前计算精确隐藏引用数量，检查 MAX_PERSISTENT_SUPPRESSION_REFERENCES=16384，成功才改变 owners/hidden。动作槽加临时 snapshot owner 的总预留受 MAX_OUTPUT_OWNERS=16384 限制；不同键数仍受 MAX_ACTOR_KEYS=512 限制。作用域预留可能在实际逻辑关闭后继续保留到整个批次退休，容量计算保守。分配器/查找耗时不具有硬实时保证。

## 原输入 Up 与清理

main 和 Held 的最后合格动作是修饰键时，作用域保留到原输入 Up。即便 `repeat=false` 使该键已经在持续时间结束时弹起、ActionPlan 没有 saved key Up，也不能提前恢复 consumed。因此时间线 payload 新增私有 ScopeUp 事件，input_up 将其排在原所有释放事件之后；它只关闭作用域，没有虚构 key Down/Up、没有新 owner。提前原 Up 仍按 FIFO 排在持续时间之后；重复 input_up 仍先遵守既有 DeadlinePending 约束，已处理后才返回 false。

最后动作不是修饰键的 main/Held，作用域在全部初始排队动作执行后恢复逻辑状态；最后 held 键仍等原 Up。其余 extra 分支全部 tap，完成后关闭。修饰键物理输出的刷新保持原惰性行为，由下一动作或原批次/控制清理边界完成，不额外插入无关同步输出。

取消及设备移除先从时间线丢弃该批次，包括 ScopeUp；只清理真正执行的 Bound，然后关闭实际作用域。未开启的作用域不发任何清理输出。reset 清空所有队列/绑定/作用域预留，原 Engine.reset 清理真正 emitted owner、变量及隐藏状态。Drop 不是 native 恢复证明。

作用域建立因精确执行预算失败时，当前事件已从队列取出，但尚未增加任何 owner/hidden。actor 锁存 failed，拒绝后续 poll/enqueue/input_up；控制取消/设备清理仍可执行，reset 才恢复普通工作。调用方必须进入原恢复路径，不能忽略错误继续输出或重试已取出的事件。

## 验证

新增 12 个确定性集成测试：首执行时恢复快照/隐藏额外键、持续时间后原键恢复；原 owner 中途释放时独立 snapshot 保持；两作用域按任一顺序取消；未执行取消无假 Up、reset 临时 owner 清理、设备移除丢未来 Up；非法 scope 元数据、时间线溢出后相同 ID 重试；精确 16384 引用上限、下一事件失败零 owner 变更/失败锁存/reset；最后 tapped modifier 无 saved Up 仍待原 Up；早 Up 等持续时间；空分支无幽灵作用域；已准入旧 Arc 跨 activate；最后非修饰键提前恢复逻辑 consumed 状态。

Linux 工作区 994 个测试用例通过，Clippy -D warnings、fmt、上游 60 个文件 hash 校验通过。初次 focused 测试把已有待执行 ScopeUp 时的重复 input_up 期待为 false；按既有 DeadlinePending 约束修正期待，并增加处理后 false 检查。初次 Clippy 提示可合并 if，已修改并重新验证。无新增 unsafe、FFI、依赖或 ABI，未重复 ABI oracle。

Windows 原生编译及之前的平台补验仍待，Linux 测试不代替 Windows 证据。本轮仅纯 actor 和既有无输入子进程检查，没有真实全局键盘、设备、驱动或安装操作。最终记录见 [验证记录](validation/linux-2026-10-11-modifier-scopes.json)。

## 后续开发

此轮只接 Output-stage scope；完整 Simple/Complex actor 仍须建立跨阶段 modifier 状态、条件生成视图和父子绑定协调，不能把终端全局 modifier 状态直接当作每一前置阶段的状态。旧选择在 activate 后生成新子批次的准入需要由该 actor 的保存绑定授权，当前 public enqueue_scoped 不放宽 Arc 门槛。Held/alone/delayed 的准入与取消、来源子树预算、simultaneous 和平台接入仍待。生产 Engine 的 UnsupportedActionTiming 拒绝保留；不得用 scoped 终端测试宣称 native duration 或完整移植完成。
