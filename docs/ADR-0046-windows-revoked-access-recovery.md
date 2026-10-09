# ADR-0046：metadata / lease 失效也禁止恢复提交

ADR-0045 将 WTS 消息锁存接入 submit/recovery，但没有消息时，metadata / AccessLease 错误仍会在 shutdown 中盲发已接受的 ups。恢复提交仍受当前桌面、UIPI 与会话边界约束，不能因为 close Hook 已完成就假定原输出目的地可达。

选择：Hook 同线程 TLS 增加独立 ACCESS_LOST 锁存，与首个 HookFault / UNTRACKED / 通知窗口锁存分开。InputAccess、AccessLease、SessionChanged 停止均标记；monitored_wait 在读取 probe 错误后先标记，再保留已发生 callback 的首错误，避免次要 metadata 错误被主因覆盖。不从独立只读 check_input_access 诊断自动撤销捕获；只有协调器确认的观测参与这个状态。

GuardedBackend 每批实际调用之前检查该锁存，包括 disabled / unhook 后 recovery；原 API 的真实 receipt 仍确认。shutdown 在 unhook 后读取独立证据，access_lost 时使用 stop_for_access_loss_after_unhook：Reset 取消 engine tasks，known empty ledger 正常 Stopped，非空或 unknown 保留 submitted 并 Unresolved / quarantine，不注入 ups。ShutdownReport.access_lost 与 session_changed、untracked_input、primary error、Hook/window cleanup 分别报告。恢复仍没有跨桌面原子性；失效消息/观测到达之前已执行的 API 无法撤销，不冒充成功恢复。

普通 emergency/clock/engine/output 故障在没有访问失效证据时保留既有有限恢复；WTS metadata/lease 失效不以新成功查询覆盖，也不自动续租或重启。新 Hook 注册仅在允许复用的线程上初始化新状态，失效线程仍由已有 poisoning 拒绝。实际锁屏/RDP/UIPI/故障验收、独立监督/Hook 存活及完整 host 仍待完成。

没有增加 Win32 API 或改变 ABI；确定性 mock / 明确时间故障测试与实际双架构 Rust / SDK 链接检查分开记录。依据：[SendInput / UIPI 与部分接受限制](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)、[WTS 会话通知](https://learn.microsoft.com/en-us/windows/win32/termserv/wm-wtssession-change)。
