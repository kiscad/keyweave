# ADR-0033：Windows SendInput 接受量与恢复

## 选择

Windows OutputPump 独占 backend 与 OutputQueue，一条 scan packet 对应一个语义边沿。SendInput 返回插入数量，按串行插入的前缀确认；短返回舍弃未提交计划，进入 StopRequired。调用者先停止物理 capture，再显式 begin_recovery，逐批释放已知 submitted 的普通键、最后修饰键。恢复短返回只重试仍未接受的 up，恢复结束 Paused，重新启用仍须真实 neutral/会话 gate。零返回也是已知零接受，错误码只用于诊断，不能据此断言 UIPI。

与 uinput 不同，SendInput 没有进程独占的虚拟键盘。Drop backend、进程退出或卸载 Hook **不能证明已注入的全局键状态已释放**。因此 Windows pump 从不调用 OutputQueue::backend_destroyed；未知接受量/非法 receipt quarantine 保留最后已知 ledger，close 保留未解决状态，不伪造全键 up 或 neutral。Drop 不发送隐式输入。实际运行时必须显式停捕获与有限恢复，并将 unresolved 状态报告给用户；与未捕获物理键碰撞的协调仍待 Hook runtime 实现。

原生边界使用固定 Win32 INPUT/KEYBDINPUT/MOUSEINPUT/HARDWAREINPUT 布局及 extern system，cbSize 为完整 INPUT 大小。只允许已核验的 x86/x64 Windows ABI；结构和常量有本地布局测试，Windows CI 必须另行运行。safe backend 在 OS 调用前整批校验 scan/flags/marker，最多 256 个。SendInput 后立即取得诊断，不把 target application 收到输入当作已证明。marker 不用于鉴权，native backend !Send/!Sync，后续由 Hook 消息线程持有。

## 证据与限制

[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) 说明返回数量、串行插入、UIPI 不可可靠诊断，以及现有按键状态可能干扰；[INPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-input)、[KEYBDINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput)、[MOUSEINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput) 为 ABI 来源。

mock 验证前缀/零返回/恢复短返回、错误与未知接受量、不重发 down、close/Drop 不清全局状态、未编码键的入队原子性。Linux 布局/mock 测试不证明 Windows 链接、UIPI、桌面切换或真机注入；Hook/同步 capture/物理碰撞/监督仍是 D08 未完成部分。原生 API 不在测试中执行真实注入。
