# ADR-0036：Windows deadline 唤醒与时钟

新增 SessionClock，以同一 Instant epoch 提供 checked u64 单调纳秒；clone 保留相同 epoch，不能给 Hook handler 与 control 各自新建 epoch。SessionControl 在消息循环外以 fresh 时间 tick actor，再取最近 output/engine deadline，返回 WaitPlan。等待毫秒向上取整、最多 20ms housekeeping，已到期为 0；不以墙钟/sleep 推测 ready，也不把等待次数当作进度监督。

HookSession 的 poll_with_control 每次先推进 control、最多取出 64 条消息，满批立即交还外层；空闲再推进 fresh control 后调用 MsgWaitForMultipleObjectsEx。零 native handles、QS_ALLINPUT、MWMO_INPUTAVAILABLE，避免旧消息被视为已看过而忽略；PeekMessage 可交付 sent Hook，期间不持有 actor/dispatcher borrow。控制错误/panic、非法 wait 返回、WAIT_FAILED、callback fault 均先禁用捕获，保存诊断、禁止旧线程复用。WM_QUIT 返回 Quit；host 随后 close，再 stop_after_unhook。此 primitive 不隐式恢复，也不安装 Hook 做测试。

通过同源 mock Win32 API 验证有界消息批、控制异常、输入可用/timeout/失败、fresh 最近 deadline 与无新按键的实际 Engine held 动作。SessionClock 真实 Instant 读取单独验证；Windows 生产 ABI/链接/实际消息等待仍需独立执行。20ms 上限不是延迟保证，睡眠/桌面切换需 session 协调；重复源、neutral、碰撞、紧急组合、监督和可执行 worker 仍待完成。

来源：[MsgWaitForMultipleObjectsEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)、[PeekMessageW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-peekmessagew)。
