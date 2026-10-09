# ADR-0048：Windows owned Job 与挂起启动

后续状态：创建后 Assign 的顺序由 [ADR-0049](ADR-0049-windows-atomic-job-startup.md) 替换为创建时 Job 属性准入；以下保留本切片的历史设计与验证边界。

Windows supervisor 必须先取得独占退出控制再让 worker 执行；std::process::Command 配 CREATE_SUSPENDED 却不保留 primary thread，不能用 undocumented NtResumeProcess 绕过。采用窄 native CreateProcessW / Job Object / ResumeThread 边界。

顺序：创建未命名、不可继承的 Job，设置 KILL_ON_JOB_CLOSE + ACTIVE_PROCESS_LIMIT=1（不允许 worker 自建子进程），用明确绝对 .exe application path、可写 bounded UTF16 command line、无 handle 继承与 CREATE_SUSPENDED|CREATE_NO_WINDOW 创建进程；成功 AssignProcessToJobObject 后才返回可 resume 的 owner。主线程必须恰好从 suspend count=1 恢复，异常或 API 错误要求停止，不能推断程序 ready。

spawn failure 保存 primary / rollback error，已有进程在 error.owner 中保留所有权（Prepared 表示尚未创建进程，其他状态含 Exited / termination pending）；禁止只关句柄而遗弃 suspended child。未分配的进程只能 TerminateProcess 自己创建的句柄；已分配则 TerminateJobObject 自己未命名的 job。request_stop 是请求，poll_exit 以有界 WaitForSingleObject 的 signaled 结果和 GetExitCodeProcess 证明退出；exit code 259 也可为真正退出值，不用 STILL_ACTIVE 猜。没有任意 PID kill、handle export/Clone、全局继承或 breakaway。

Drop 是有限停止/等待/关闭 fallback，不声明已退出；显式 owner/primary error 必须在 host 保持至实际 exit。Job 配置只约束 kernel-owned child 生命周期，不是安全沙箱/进度证明或输入恢复。默认使用当前进程用户 token / 环境 / cwd；不能把 arbitrary user config 当 trusted worker command，正式 host 仍需可信路径、精确环境/句柄列表与私有 IPC 启动契约。

target Windows10/11 x64/i686，startup/job/process repr(C) 与 WINAPI prototypes 用 SDK oracle 校验；mock 次序/失败/ABI/命令行 tests 与实际无输入资源 smoke 分开。尚无 native transport/Job 监督器、parent crash 验收、Hook liveness或完整 host。Job handle 最后关闭有 kill-on-close 属性，但真实 parent death 行为仍须独立进程验收；不得用配置位冒充实测。

依据：[CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)、[CreateJobObjectW](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-createjobobjectw)、[Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)。参数编码不用 shell，不自行提权或加载驱动。

恢复调用前锁存 ResumeAttempted，即使停止回滚失败也不能再次 ResumeThread；调用者保留 owner 重试停止/观察退出。创建到 Assign 之间存在父进程崩溃导致挂起子进程遗留的窗口；该子进程尚未执行/打开输入，但资源回收仍不完整。正式 supervisor 启动须用支持版本的 PROC_THREAD_ATTRIBUTE_JOB_LIST 原子入 Job 或等价已验证启动契约消除此窗口，不能把本切片描述为完成父死亡保护。
