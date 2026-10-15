# ADR-0230：Windows 公开控制的主体与明确 SID DACL

公开控制不能直接复用worker私有progress pipe的随机名字/OW DACL或将任意PID当已认证peer。先实现完整的冷阶段主体快照和权限描述符资源，为后续公开named-pipe transaction提供必要基础；本切片不宣称Windows Status/Stop端点已接通。

主体为TokenUser完整SID、TOKEN_STATISTICS.AuthenticationId登录LUID及TokenSessionId三元组。每次读取都要求调用线程无impersonation，process token为primary，非session0；统计TokenId/AuthenticationId/ModifiedId前后相同才接受，返回长度/SID指针范围/版本/子authority计数严格检查。固定1024-byte、8字节对齐缓冲，不扩大或截断。SID文本来自ConvertSidToStringSidW；以独立有界SID字符串解析还原binary并比较，避免错误转换或SDDL注入。Debug不泄露主体值。

公开ControlIdentity.capture只读当前进程；same_primary_token_scope_of是只读process-token比较，不是pipe-peer证明。进程句柄只以PROCESS_QUERY_LIMITED_INFORMATION打开且GetProcessId前后核对，调用者主体也前后重验；不能据此授权任意caller-provided PID，后续pipe必须绑定内核报告的peer和实际消息连接生命周期。

ControlIdentity创建protected DACL只含实际用户完整SID和SYSTEM，不用OW/AU/Everyone/Administrators泛化。描述符由SDK创建并独占LocalFree，raw指针仅native内部短借用，不导出/clone/继承；token/process/文本回收失败独立保留原错误，线程sticky poison阻止继续开户，未知只读handle保留至进程exit，不冒称清理成功。描述符显式close失败仍保owner供有限重试，Drop不证明回收。callback中拒绝capture/query/create/free。

该权限模板没有自动验证peer登录/桌面，不能代替named pipe的REJECT_REMOTE、FIRST_INSTANCE、非继承、SQOS、peer binding、OVERLAPPED稳定内存与取消完成证明。旧worker私有管道保持不变。Windows-only实现沿项目现有Linux API-mock方式验证生产路径；新增SDK C oracle静态断言不等于实际MSVC/SDK已执行，先前Windows原生审批额度拒绝不重试。

依据：[GetTokenInformation](https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-gettokeninformation)、[TOKEN_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-token_statistics)、[ConvertSidToStringSidW](https://learn.microsoft.com/en-us/windows/win32/api/sddl/nf-sddl-convertsidtostringsidw)、[SID字符串](https://learn.microsoft.com/en-us/windows/win32/secauthz/sid-strings)。认证/授权、PID复用/转交handle与身份变更的实际Windows对抗验收仍是公开transport后续gate。

后续实现顺序：

1. 先定义独立公开 message-pipe transaction：Connect → Read 一条 32-byte request → Write 一条 64-byte reply → 有界观察 client close → Disconnect → rearm。Windows duplex 不具备 Unix 写半关闭语义，不复制 request+EOF 协议，不用阻塞 FlushFileBuffers 等待客户端。
2. 在实际连接上绑定内核 peer 与生命周期，审查 PID 复用、句柄转交及身份变更；本次 process token 比较不能单独作为授权依据。
3. 为公开 transport 单独复用或提取 OVERLAPPED 稳定内存和取消完成规则；未证明完成不得 reset/free，保留 FIRST_INSTANCE、REJECT_REMOTE、非继承和 SQOS。实际 factory 接入时再加入描述符内部同步短借用接口。
4. 接通既有 KWCC/KWSC DTO、host 原 Supervisor、CLI 和 GUI；每轮先做监督时限/exit，再有界处理公开 IPC。任何 Status/Stop 请求都不产生 worker progress 或续租。
5. 实际运行 MSVC x64/x86 SDK oracle、生产 native_link 和无输入 named-pipe 正反例后才能声明 Windows 控制可用；Hook/注入/硬件验收另行保留。

本切片验证：Linux 全量 1949 项通过，8 项已有 native IPC 默认 ignored 未重跑；新增 11 项为纯 SID 和实际生产身份路径 API mock。格式、Clippy、源码 62、现有 Linux/X11 ABI 与 regex oracle 通过。首次失败来自 mock 未读取 current_session fixture，已修复并加入 GetProcessId 前后不一致等负例。新 Windows SDK C oracle 与 native_link 尚未原生验证。见 [验证记录](validation/linux-2026-10-16-windows-control-identity.json)。
