# ADR-0037：Windows 桌面与按键采样的访问前提

GetAsyncKeyState 零返回可能是未按住，也可能是 inactive desktop、UIPI 或缺少 desktop Hook 权限。新增只读 sample_input_state，先检查可见 WinSta0、当前线程 desktop 的 UOI_IO、OpenInputDesktop(READOBJECTS|HOOKCONTROL) 与 thread/input desktop 名称，同时拒绝线程 impersonation，再检查 foreground process token integrity 不高于当前进程；采样 VK 1..254 只使用 high bit，忽略不可靠 low bit，之后重验完整上下文。无 foreground/权限/字段/上下文变化全部失败；包括鼠标 VK 是保守的 startup 限制。不在 Hook callback 查询异步状态。

元数据获取只用固定 64 WCHAR 名称与 256-byte aligned token buffer；TOKEN_MANDATORY_LABEL 的 SID 指针必须落在已返回 buffer 中，验证 S-1-16 与单个 RID，不解引用未验证指针。只对自己 OpenInputDesktop/OpenProcess/OpenProcessToken 得到的 owned handles 做 Close；GetThreadDesktop/GetProcessWindowStation/current-process/current-thread 句柄是 borrowed，不关闭。成功路径 owned close 失败也拒绝采样；错误路径有限 RAII cleanup 保留主错误。不修改桌面、ACL、token/integrity，不自动提权。

返回 Held 或 IdleObserved，**不签发 capture-ready/neutral token**。这是逐个 VK 与 pre/post 上下文的有限样本，不能证明原子无按住，也不能排除在两次检查间发生又恢复的桌面/foreground 变化。必须与 passive Hook 的完整 down/up 绑定、安装前后检查、监督及启动事务结合后启用；函数不调用 actor.start_after_neutral、不安装 Hook、不注入。正常活跃事件循环不得周期性全 VK 扫描；应用/桌面切换恢复有独立 gate。

全部 API 替换测试覆盖正/负访问、low/high bit、UIPI integrity 拒绝、metadata/token 格式错误和采样途中 context 变化；SDK oracle/生产链接入口同步新增结构/常量/原型。当前 Linux 只验证 mock/布局，不证明真实 Windows 访问或 neutral。

来源：[GetAsyncKeyState](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getasynckeystate)、[GetUserObjectInformationW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getuserobjectinformationw)、[OpenInputDesktop](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-openinputdesktop)、[GetTokenInformation](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation)、[TOKEN_MANDATORY_LABEL](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-token_mandatory_label)、[Mandatory Integrity Control](https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control)。
