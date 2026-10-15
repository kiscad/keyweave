# ADR-0210：Windows 鼠标 Hook 解析与保存捕获决定

新增不执行 OS 操作的 MouseHookRecord 解析与 MouseDispatcher。消息白名单为 movement、五按钮Down/Up、两轴wheel；wheel取mouseData高16位signed值、不换算120；XBUTTON高word1/2映射单按钮；官方允许组合3，本阶段明确报告CombinedButtons unsupported并停止/pass，尚未实现原子多按钮捕获决定，不称为非法Win32记录；其他usage拒绝。未定义mouseData位忽略，原flags只允许注入bit0/low-integrity bit1且low必伴注入。marker仅与injected结合辨识own，物理同marker仍physical；不是鉴权/设备归属证据。所有坐标保持原screen x/y。

MSLLHOOKSTRUCT.pt 是 per-monitor-aware screen coordinates：[官方结构](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msllhookstruct)。不能将前后坐标差直接作为raw relative input重注入，否则速度/加速和多显示器坐标改变可能重复应用；本阶段Position强制pass，handler请求Capture会停止dispatcher并报告MovementCapture。该handler仍可观察物理movement用于取消alone/滚动资格，实际Runtime后续接入。wheel/button允许同步显式capture；所有own/other injected默认pass、不进规则或pressed slots。

五个固定button槽保存首次Down决定；后续重复Down与Up带原binding并使用原决定，orphanUp pass。handler故障/panic锁存、保持旧slots并pass，不伪造release/neutral。nCode非0不读record、不调用handler。panic payload泄漏一次避免析构二次panic穿越callback边界，沿用键盘dispatcher。无IO/配置解析/时间查询；不是原生Hook注册或物理设备识别。

[LowLevelMouseProc](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelmouseproc) 的callback边界仍需后续独立owned registration、thread poisoning及session/runtime监督，默认键盘捕获不变。
