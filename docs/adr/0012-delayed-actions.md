# ADR-0012：松开后延续的 delayed_action

状态：已实现离线 delayed 切片；2026-10-08。

to_delayed_action 包含 to_if_invoked/to_if_canceled，默认 delay 500ms。up 不取消 delayed；任务保留原配置、设备、down 修饰快照和预算，到期 invoked 完整轻触，不替换正常 to。另一个新 key/button down 或同阶段派生 down 执行 canceled（在新匹配/输出之前）；repeat/重复 down/wheel/move 不取消。两分支只执行一个；动作 conditions 在执行时读取当前上下文/变量，配置是原快照。

源按键 up 后若仍有任务，将其 binding 预算转为 pending binding，释放普通输出但保留 token cursor/配置；最后任务完成后回收。active+pending 总数上限仍 4096，SourceToken 总 reservation 上限 16384。原 down 展开包含 invoked、canceled、快照 restorer、scope 和 token；热更新不能侵占旧任务。每个控制 timer 单次完成一个任务。

当新 down 加取消宏的保守预算超过单事件 1024 edges 或需要先回收 pending 占用的 binding/owner 容量时，返回 CancellationPending，seq/time/输出不变；运行时按 next_delayed 提交 CancelDelayed{token}，再提交原输入。普通情况在 down 内完成取消。到期优先执行 TimerFired，CancelDelayed 不能绕过已到期任务；Reset/remove 无宏优先撤销。runtime 的统一 seq 分配规则同 ADR-0011。

delayed 运行时恢复 down 的标准 modifier 快照、临时消费 mandatory，然后恢复当前逻辑状态；任务结束且源设备没有任何 active physical binding 时刷新 latent modifier 状态，避免独立延迟宏留下虚拟修饰键。这沿用 KeyWeave 的设备 neutral 契约，不宣称复现所有上游惰性 dispatcher flush 细节。

设备移除/代际替换/Reset 删除 active 与 pending 的任务，不执行 canceled/invoked；旧回调无输出。pending 不算 physical down，不阻碍新的 neutral 判断。变量状态仍按 ADR-0010。没有线程/sleep/OS clock、delayed expressions、simultaneous 或 hold duration。

当前按住/延迟只允许 complex/basic，直接构造 simple timed IR 返回 InvalidMapping，与配置编译器的 simple 字段范围一致。这样 pending 的单阶段任务不会递归建立新的 delayed 任务；同一物理 identity 在重新按下前完成旧取消，不会覆盖旧 cursor。simple 产生的 complex 规则及其延迟任务仍受完整预算保护。

验证：14 个 engine、4 个 compiler、1 个 CLI 新测试，workspace 总计 152 个通过。覆盖松开后保留/触发、取消顺序和原 to、不取消事件、旧配置/变量 cursor、移除/Reset/重连、held 并存、down 修饰快照和 idle、due 与 canceled 顺序、构建中的同阶段派生取消、996/994 edges 分批、2000 次预算回收、4096 binding 容量、当前设备条件与 Unknown/过期 unless。两个 CLI fixture 分别结束在 drained/pending，next_deadline 返回时间/token。

来源：固定 to_delayed_action.hpp（原已在 manifest），setup 的 shared_ptr 保留说明 up 后延续，cancel 只检查 key_down，post_events 的 scoped restorer、完整 extra taps 与清 current pointer 已静态核对。上游预计输出未修改；没有执行 macOS harness。真实 backend timer、失败提交与 watchdog 仍未实现/验证。
