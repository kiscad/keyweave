# ADR-0034：Windows Hook 的同步绑定与线程边界

新增 HookDispatcher，固定 512 native slots，首个物理 down 同步调用 handler，保存 Capture/Pass 到 Repeat/up；后续返回不能改变旧决策。无法解码的标准/特殊/歧义 scan 整个 down/up 生命周期放行，不把后续布局 VK 当作新匹配。孤立 up 放行；自身和其他 injected 在任何 mutable handler borrow 前放行。nCode 非 HC_ACTION 不读取事件数据并调用后续链。

handler 错误、非法物理 record、panic（unwind 模式）使 dispatcher 永久停止捕获、当前事件放行，保留绑定用于故障审计。panic payload 不执行任意 Drop；不从 FFI unwind。此时必须由外层停 Hook、恢复 output ledger，不能把 fail-open 描述为全局注入状态已释放。handler 只允许有界同步逻辑；没有先 queue 后放行再映射。

原生 HookSession 安装 WH_KEYBOARD_LL、持有安装线程消息循环，TLS 保存唯一 dispatcher。callback 在读取 OS 借用 KBDLLHOOKSTRUCT 后先 injected bypass，再 try_borrow；物理重入停止 session。故障 PostQuitMessage 唤醒 GetMessage；消息循环明确区分 -1/0/正返回，退出禁用捕获。unhook 前禁用 callback，关闭后取回 handler 供外层有限恢复。Unhook 失败或 callback/message 故障保存诊断并禁止本线程重新注册，避免旧静态 callback 误用新 session；Drop 仅停 Hook，不注入/清输出。

此组件不是完整 engine/output/session coordinator：neutral、未捕获物理键碰撞、deadline 驱动、紧急组合、进度监督与退出时有限恢复仍需接入。无可信设备 ID；没有 Hook 存活证明，silent timeout removal 不显示为健康。只编写接口，不在开发测试中安装全局 Hook。

来源：[MSG](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msg)、[UnhookWindowsHookEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-unhookwindowshookex)、[LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc)、[SetWindowsHookExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowshookexw)、[GetMessageW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getmessagew)。Linux portable tests 与布局检查不证明 Windows 编译、安装成功、安全桌面或真实拦截；Windows 分支及真机须独立验收。
