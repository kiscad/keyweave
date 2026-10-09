# ADR-0044：WTS 会话身份与活动访问检查

既有输入桌面/WinSta0/integrity和AccessLease不能直接证明RDP连接状态。只检查刷新前后两次快照也不能保存启动会话的身份。需要在callback外检查本进程所属的本地WTS会话，而不是猜测当前console session或查询远程server。

选择：ProcessIdToSessionId读取caller与foreground PID所属session并要求相同、非0且非保留ID；WTSQuerySessionInformationW使用WTS_CURRENT_SERVER_HANDLE和明确caller session id、WTSSessionInfoEx。只接受SDK核验的Unicode level1布局/完整长度、返回session id一致、WTSActive及WTS_SESSIONSTATE_UNLOCK，登录时间必须有效。只读取Level/SessionId/SessionState/SessionFlags/LogonTime这些整数；不复制/解析/记录用户名、域名、client/window标题或last-input字段，也不读padding。API返回buffer唯一owned并使用WTSFreeMemory，不混用CloseHandle或Rust allocator；错误/NULL/长度/level/identity/state/lock都拒绝。

把session identity(id, logon-time)加入既有Context pre/post比较。CaptureSession保存startup identity；passive activation最后一次Hook-binding检查前完成同session metadata验证，active租约refresh也要求同identity。改变或未知不会续期/恢复旧actor。callback不执行WTS/RPC，仍只检查当前lease；metadata query中可有sent callback，保持borrow外调用、首fault优先与完成后fresh-clock/expiry检查。WTS调用可能耗时，100/500ms是调度/租约参数，不保证API墙钟上限。

Windows10/11为目标；Windows7/Server2008R2存在lock/unlock反转的文档缺陷，不将旧系统纳入支持。WTS服务不可用、查询权限拒绝、格式未知均拒绝，不自动提升权限或用desktop成功覆盖WTS失败。metadata观察不原子，检查间隔内仍可短暂切换/断连；不能替代WTS通知窗口、Hook存活、独立进度监督或完整可执行host。untracked输入保护与global output Unresolved继续保留。没有实测锁屏/RDP断连时不得宣称真机验收。

依据：[WTSQuerySessionInformationW](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/nf-wtsapi32-wtsquerysessioninformationw)、[WTSINFOEXW](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ns-wtsapi32-wtsinfoexw)、[WTSINFOEX_LEVEL1_W](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ns-wtsapi32-wtsinfoex_level1_w)、[WTS_INFO_CLASS](https://learn.microsoft.com/en-us/windows/win32/api/wtsapi32/ne-wtsapi32-wts_info_class)、[ProcessIdToSessionId](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-processidtosessionid)。具体sizeof/offset/enum与WINAPI prototype由安装SDK的x64/x86 compile-only oracle及对应Rust布局tests验证。
