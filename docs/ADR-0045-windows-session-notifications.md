# ADR-0045：WTS 通知窗口与不可恢复的访问边界

问题：ADR-0044 的 metadata refresh 只能观察查询时的状态，查询间的锁定/断连事件无法立即停止同线程 Hook；一次锁定后再解锁也不能被当作原 lease 仍有效。通知必须在不借用 actor 的 sent-message 回调中关闭新提交。

选择：CaptureSession 在 passive Hook 之前拥有独立的同线程隐藏窗口（不显示、不激活、不 subclass 外部窗口），注册 NOTIFY_FOR_THIS_SESSION。保存明确 caller session ID；自己的任何 WM_WTSSESSION_CHANGE 都锁存 access_lost，其他 session 通知忽略，未知事件仍停止。WM_NCDESTROY 意外销毁也停止。回调只处理 HWND/整数/TLS Cell，无配置/引擎/WTS/键查询；通过 Hook 的统一停止函数保留首错误，后续解锁/重连不清理锁存。独立 SessionNotifications 诊断不安装 Hook、不发送输入。

活动 backend 在每次实际 submit 前也读取锁存，包含 unhook 后的有限 recovery。已有 API 的真实 receipt 仍须确认；访问丢失时不盲发 ups，Reset 撤销 engine tasks，submitted 非空或 unknown 进入 Unresolved。已知空 ledger 可以正常停止。锁存独立于首个 HookFault，emergency 或 untracked 不能遮蔽 session change。ShutdownReport 分别保存 session_changed、untracked、Hook 清理、窗口清理与恢复结果。

资源：每个窗口拥有自己的 RegisterClassW atom、CreateWindowExW HWND 和成功通知注册；不接管借用的 module。按注销通知→DestroyWindow→UnregisterClassW 清理；错误记录且毒化线程，不能重用不确定 handle/同线程捕获。意外 NCDESTROY 已使 HWND 失效，close 跳过 unregister/destroy 并报告 unexpected_destroy，避免数字 handle 重用后误操作其他窗口。Destroy 失败不能注销仍有窗口的 class；静态 WindowProc 不引用 Rust instance/裸堆指针，失败留下的 OS 资源只能随进程结束回收。startup 失败也必须回滚已获得前缀，保留主要与清理错误。

通知不是身份认证（其他程序可伪造消息而使之停止），不授予 neutral/lease，不保证系统从变化到消息交付的最大延迟；WTS 服务未就绪直接失败，不阻塞等待或提权。周期性 metadata baseline 保留；独立监督/Hook 存活/完整 executable host 仍必做。停止后重启需全新启动与 neutral/会话核验，不以解锁消息自动启用。真实锁屏、RDP、注销和硬件恢复验收保留。

依据：[WTSRegisterSessionNotification](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsregistersessionnotification)、[WM_WTSSESSION_CHANGE](https://learn.microsoft.com/en-us/windows/win32/termserv/wm-wtssession-change)、[CreateWindowExW](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-createwindowexw)。SDK oracle 与 x64/i686 Rust tests 检查 WNDCLASSW 布局、constants 和 WINAPI prototypes；mock fault injection 与实际只读注册/注销 smoke 分开记录。
