# ADR-0051：挂起 Windows worker 的精确进度端点交付

OwnedJobProcess 增加接收 owned PipeReader/PipeWriter 的启动入口。两端必须来自不同 pair、没有在途操作或已缓存数据且仍可用；它们只用于二进制进度协议，不接受任意 raw handle。创建时同时安装 JOB_LIST 和 HANDLE_LIST，后者仅含 worker stdin reader、NUL stdout、worker stderr writer。Job、父端管道、OVERLAPPED events、进程与线程句柄均不进入列表。所有属性值在 stable heap 中保持到 Delete，bInheritHandles 仅此入口为 TRUE。

端点暂时设置 HANDLE_FLAG_INHERIT，在子进程仍挂起时删除属性、清继承位、关闭父进程的 worker 端及 NUL；全部明确成功后才允许 ResumeThread。创建或收尾失败保留 primary、逐资源清理报告与 partial owner，并请求停止自己的 Job；已创建 child 的启动失败先锁存 StartupFailed；停止失败后即使清理重试成功也永久拒绝 resume。发出终止请求不是实际退出。保留 owner 可重试清理及 poll_exit，Drop 仍只提供有限 fallback。已被转移的 pipe 不再交还调用者，也不重复提交在途 IO。普通无通道 Job 启动保持原行为。

该启动控制线程不得与会使用环境继承的其他进程启动并发；短暂的 inheritable 窗口无法约束不遵守精确列表的外部库。后续用户命令应由独立 session agent 执行。!Send/!Sync 约束 owner 的使用线程，不代表全进程其他线程不存在。worker 必须在使用端点/打开输入前执行独立启动核验，随后接入真正 progress supervisor；本启动组件不替代它们，不证明 Hook 存活或全局注入键已释放。

依据：[UpdateProcThreadAttribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)、[SetHandleInformation](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-sethandleinformation)。SDK oracle 与实际无输入子进程验收分别记录，不能用 mock 代替跨进程运行。

ChannelCleanupReport分别保留三handle的继承位错误、reader/writer的完整PipeCloseReport与NUL关闭错误，所有收尾尝试均执行；JobSpawnError中的历史报告不被后续成功重试覆盖。错误与partial owner在非热路径boxing，避免Result错误体携带两个大endpoint。两条管道的随机identity只用于拒绝同一pair，不是对父身份的认证。

check_job_progress自身fixture不读取协议：挂起时写入24-byte challenge且reply read仍pending，证明父进程worker副本已关闭后child持有两端；resume后实际exit259并观察EOF。它不提供跨进程receipt/poll活性验收。check_windows_job.ps1 -ProgressDelivery运行该例程和既有owned Job测试；父死亡fixture改为精确进度端点启动，观察worker随own parent强制退出，用以验证没有Job handle继承保活。所有操作无Hook/SendInput/设备。
