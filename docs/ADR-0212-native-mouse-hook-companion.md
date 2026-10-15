# ADR-0212：同线程 owned 鼠标 Hook companion

现有 HookSession 显式 install_mouse_handler 为同一消息线程安装 WH_MOUSE_LL=14，复用同一tag、fault、guard、poison及message/control循环。独立TLS MouseDispatcher，无裸handler指针，registration必须已有活动键盘owner；失败归还handler。默认install不安装鼠标，现有keyboard-only Runtime不拓宽。

HC_ACTION才复制OS-owned MSLLHOOKSTRUCT，nCode非0原样forward且不解引用。own/other注入先于dispatcher可变借用过滤；任何物理cross-dispatcher reentry停止共同TAG/poison。鼠标handler/解析错误进入HookFault.Mouse并由既有控制循环观察。guard启用后外部注入或透传按钮视为untracked；Position/wheel透传不引入held状态，不按keyboard的任意Pass规则误报。所有fault/quit/close都通过共同TAG禁止后续两种捕获。

close_with_mouse返回两个handler和各自unhook结果；先同时禁用再逐个unhook，鼠标失败仍尝试键盘。legacy close仍回keyboard handler并drop mouse handler，双失败用UnhookBoth保留两个码；显式调用者应使用完整报告。unhook失败永久poison但handler仍归还，callback只访问TLS且disabled，不悬垂。Drop不注入任何release，不证明实际global中立。

Linux native-api mock测试真实callback/owner/故障/交叉重入与ABI字段，Windows SDK/真实registration尚未验证。官方资料：[MSLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msllhookstruct)、[LowLevelMouseProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc)。实际session/mixed Runtime与lease/neutral/恢复仍需接线，不能仅凭安装或消息线程存活当输入活性。

重复close的已关闭owner必须在操作TLS前返回，不能禁用同线程后来新owner。callback内close先Busy且保owner/handles、停止共同捕获，调用者应在退出callback后再close；若callback内Drop，不能宣称实际unhook完成，TLS静态callback禁用和线程poison仅是有限兜底。held_mouse_buttons仅追踪已观察的bindings，实际五按钮neutral采样仍待接。
