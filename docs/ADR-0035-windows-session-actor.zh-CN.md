# ADR-0035：Windows 会话引擎与输出协调

SessionActor 独占 Engine/OutputPump，使用单一合并设备身份（不是物理键盘 ID）、显式单调时间和递增 seq。启动/热配置先校验全部动作分支与 modifiers 的 scan 输出，拒绝设备条件；原 Engine 的 duration gate 保留。完整标准 scan 输入采用捕获并重新输出的策略，因此未映射键也捕获，不能在放行物理键后重发 passthrough。HookDispatcher 决策绑定与 actor 的 Capture 必须一致。

每次 key/tick 共最多 64 次引擎/发送操作；已有 due output 优先，再到期 TimerFired，然后本次按键。CancellationPending 分批取消并重试同一键，不跳过 timer 或生成新 source。所有 due 发送在同步返回 Capture 之前完成，未来输出保留 earliest deadline；没有 queue 后先放行。工作预算/时钟/Engine/输出错误停止 actor，外层必须立即停 Hook，然后显式 stop_after_unhook 和最多 8 次 recovery。Engine reset 输出不用于恢复；以实际 accepted ledger 为准。不能完成或未知接受量保留 Unresolved，不能靠 Drop/close 清按住状态。

ActorHandler 使用同线程 Rc<RefCell> 连接有界 HookHandler 和显式 clock provider；try_borrow 失败停止 callback，不等待锁。调用者保留 actor 引用用于 unhook 后有限恢复。native callback 已在 mutable borrow 前排除 injected，避免 SendInput 自身重入；clock provider 不得等待/访问 IPC。Engine 忽略物理 Repeat；Windows 独立重复源尚未接入，不声称长按重复可用。实际 native timer wake/neutral/特殊键和未捕获物理键碰撞/紧急组合/进度监督仍待接入，因此不能描述为完整可执行后端。

验证使用实际 Engine、HookDispatcher、OutputQueue 和模拟 SendInput，覆盖 passthrough 无双输入、a→b、repeat 不重播宏/旧配置 up、held deadline、短发送恢复、未知接受量/零接受恢复预算和配置原子性。不安装 Hook、不执行系统输入。Linux 验证不证明 Windows 编译/SDK ABI/真机。


后续实现：Windows physical Repeat 已由 ADR-0042 接入 actor，按当前原 binding 保留末端发送一条 Repeat；不重播宏或增加 owner，普通 Engine 仍不生成 Repeat 输出。本文“独立重复源尚未接入”描述首次实现时的状态，当前进展以 STATUS/ADR-0042 为准。
