# ADR-0233：Windows 最后消息 token 鉴权与服务端 transport

在 ADR-0232 消息 owner 上接入 ADR-0231 ServerTransport，服务端完成读取后才能鉴权。owner 只为完整 32-byte、非 MORE_DATA 的消息设置一次性内部资格；断开、再读或鉴权会消费它，不导出 handle。鉴权使用实际 server handle 的 ImpersonateNamedPipeClient，读取最后消息对应 thread token 的完整 SID、AuthenticationId 和 session，与启动 caller 主体精确比较，不拿客户端任意 PID 代替。

进入前要求当前无 impersonation、caller 主体仍与 baseline 一致；thread token 必须 TokenImpersonation、SecurityIdentification（client SQOS 契约），固定缓冲/长度/SID范围与 token stamp 前后检查保持。只在该作用域查询 token，不调用业务处理器、文件/进程访问、用户 callback 或等待；恢复线程后重验 caller，才返回 bool。所有 query/解析/关闭错误独立返回，绝不因失败退回 process token。

成功 impersonation 后立即设 RAII 恢复 guard，normal/error/panic 全路径调用 RevertToSelf。恢复失败不能返回 host 继续工作：生产直接 abort，不在未知身份下日志/清理/回调；mock 用可捕获 panic 验证不返回正常路径，不实际中止测试进程。这是安全边界，不能声称 abort 已恢复输入，原 worker 的独立监督/owned Job 策略仍需真实 OS 验收。

ControlServerTransport 自持 baseline 与 message owner；鉴权前禁止 write，失败不执行 handler，disconnect 后清资格。它实现纯事务的真实生产 adapter，实际 Win32 仍由当前 Linux API-mock 验证；没有因为 trait 已实现就宣称 Windows 系统管道/消息身份实测通过。客户端实际 server 身份验证、host/CLI/GUI 接入、SDK x64/x86 与真实无输入对抗验收继续。

依据：[ImpersonateNamedPipeClient](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-impersonatenamedpipeclient)、[RevertToSelf](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-reverttoself)、[pipe impersonation](https://learn.microsoft.com/en-us/windows/win32/ipc/impersonating-a-named-pipe-client)。微软明确要求 RevertToSelf 失败时关闭进程。本切片不重试既有 Windows 原生审批额度拒绝。

验证：新增 7 项，覆盖 SID/logon/session 不匹配、primary/anonymous/更强 impersonation token 拒绝、损坏 SID/长度/统计/stamp、OpenThreadToken/各 query 失败、独立 token close/LocalFree 错误、panic unwind 与不可恢复 Revert、caller 前后变化/已存在 impersonation/callback 零 API、提前/重复/截断鉴权，以及 Server machine→真实 adapter→owner→token API mock→synthetic handler 的准入/拒绝链。完整 Linux workspace 1980 通过，8 native 默认 ignored；格式/Clippy/源码 62/现有 Linux 与 X11 ABI/regex oracle 通过，无初始失败。新 SDK/static link 项只写入尚未原生运行，生产 abort endpoint 没有真实执行。见 [验证记录](validation/linux-2026-10-16-windows-message-authorization.json)。

下一步实现实际客户端 server 主体/存活绑定（审查 PID 复用与 handle 转交），再接入 ControlClientTransport，随后 host 原 Supervisor 与 Status/Stop CLI/GUI。服务端目前是 per-user/nonzero-session baseline，不能直接作 session0 跨用户服务。实际 Windows no-input named-pipe token/句柄转交/恢复失败与 x64/x86 SDK gates 均不得由本 mock 绿灯替代。
