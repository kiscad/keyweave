# ADR-0043：未归属输入边界与发送重入停止

当前 Windows 完整标准 scan 捕获模型掌握自己的 accepted ledger，但外部 injected 与 unsupported physical 输入仍旁路；前者不可信每事件设备来源，后者可能有别名/特殊序列。不能用扫描码或VK猜测全局状态所有权，更不能在这些输入改变状态后盲发已知ledger的up并宣称neutral。

本次接入的是完整捕获会话的准入/停止保护，不声称已经实现外部输入共存或逐键碰撞协调。CaptureSession从passive切换为active时启用native TLS guard；own injected按原marker+flag放行，other injected在dispatcher可变借用之前标记未归属并停止。健康dispatcher最终Pass的physical事件也触发；故障导致当前physical down/repeat放行，或无法验证的事件放行时，同样记录风险（有效captured up故障不建新down）；捕获已停止但尚未close期间的后续非own输入仍标记。guard只存无按键信息的sticky bool，与首个HookFault独立，避免其他故障遮蔽恢复风险。Standalone HookSession默认不启用这个会话策略。

会话backend封装在每次active原生提交前检查同线程fault。SendInput期间外部注入重入时，不借用actor/dispatcher就能停止；本次API实际receipt仍按原规则确认，下一批在调用backend前Rejected，不继续注入未来down。不是把已调用API伪装为零接受。关闭先禁用capture与guard并unhook，随后：没有已提交输出且接受量已知时，取消未来计划并正常Stopped；有已提交按住或已有unknown quarantine时，不发恢复up，保留账本、取消Engine任务并Unresolved。ShutdownReport明确标记观察到untracked input，首个主错误与unhook错误保留。不能靠Drop、close或进程退出清全局状态。

这种行为会让active会话在外部自动化/unsupported键到来时停止；这是当前全捕获模型对未归属来源的明确能力限制，仍须后续共存/逐键协调及真机测试。guard无法看到尚未注册、已经unhook或被系统静默移除期间的输入，也不能证明close与恢复之间不存在外部事件；原neutral采样非原子、marker可伪造、同scan多键盘合并与Hook存活/独立监督限制继续保留。停止保护不能替代这些完整开发目标。

依据：[Microsoft SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) 说明现有键状态干扰、返回插入量与串行插入；[LowLevelKeyboardProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc) 说明安装线程消息交付与系统可能静默移除；[KBDLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct) 定义injected flags。本项目不从这些接口推断可信每设备所有权。

紧急退出与时钟等故障仍保留首个原因，但若该回调同时放行了physical down/repeat，则untracked风险可能使恢复Unresolved。普通没有旁路/风险的emergency仍走close-first有限up恢复；两种情况必须分别测试，不能用“紧急成功”抹掉global-state归属不明。

验证：新增12个session和3个actor行为测试；Linux497 tests及独立worker harness、fmt/clippy/source verifier通过，Windows1.97.1 x64/i686各460 workspace tests（126 Windowsunit与1 production link）、clippy和SDK oracle通过。两个128文件hash快照无差异，见 [Windows证据](validation/windows-2026-10-09-untracked.json)。全部Win32 input APIs在unit tests替换，未实际安装/注入。
