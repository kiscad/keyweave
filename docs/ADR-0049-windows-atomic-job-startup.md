# ADR-0049：创建时原子 Job 准入

ADR-0048 的 CreateProcessW → AssignProcessToJobObject 顺序让挂起 worker 在两次调用之间尚无 Job。父进程若在该窗口死亡，worker 尚未执行输入代码，但可能永久遗留。以 Windows 10/11 为目标，改用 STARTUPINFOEXW 的 PROC_THREAD_ATTRIBUTE_JOB_LIST，在创建事务内准入；不提供旧顺序回退，也不使用 breakaway。Job 无法准入、父进程所在 Job 的嵌套限制等情况必须使创建失败。

启动流程：创建 private/noninheritable unnamed Job → 设置 kill-on-close/active=1 → query/init/update 一个 Job 属性 → CREATE_SUSPENDED|CREATE_NO_WINDOW|EXTENDED_STARTUPINFO_PRESENT 创建 → 保留 process/thread ownership → 返回 Suspended → 显式 ResumeThread。成功创建的进程全部已属于自有 Job，不再调用 AssignProcessToJobObject 或 TerminateProcess；停止只作用于自有 Job。仍然无 handle 继承/导出/复制；属性中的数值是借用同一 Job，不是 DuplicateHandle。

属性 list 的 opaque buffer 从官方 sizing query 得到，只接受预期 ERROR_INSUFFICIENT_BUFFER 和 1..4096 bytes；使用稳定 heap storage、16-byte alignment，覆盖 Windows x64/x86 [HeapAlloc 对齐](https://learn.microsoft.com/en-us/windows/win32/api/heapapi/nf-heapapi-heapalloc)的要求。Job HANDLE 数组另在稳定 heap allocation，直到 DeleteProcThreadAttributeList 才释放，不能使用构造器局部 stack array。只有成功初始化后才 delete，所有 update/create 错误路径恰好一次；query/init 失败不 delete。cb=sizeof(STARTUPINFOEXW)，保留原 STARTUPINFOW prefix ABI。资源 owner 在属性清理期间持续拥有 Job。

Prepared 仅说明 Job 存在、没有成功创建子进程；创建失败不会编造 Exited/termination。ResumeAttempted 的不可重试约束和 request_stop/poll_exit 的分离不变。Drop 有限 fallback，显式进程退出仍必须观察。Job 不证明输入账本恢复，也不是安全沙箱；本更改不授予捕获权限。

无输入父死亡验收由 tools/check_windows_job.ps1 驱动现有 check_job fixture：测试自己启动父 fixture，父只启动当前 exe 的 ready/park worker，worker 在本次唯一临时 marker 中写自身 PID。观察器只用 PID 获取并保留观察句柄、验证当前 exe，确认其存活后终止自己所拥有的父 Process；通过保留句柄等待原 worker 实际退出，避免 PID reuse 充当退出证明。不按 marker PID kill，不依赖 Rust Drop，不安装 Hook/注入/加载驱动。此验收覆盖已恢复 worker 的父死亡；不会枚举所有内核调度切点，也不证明全局按键已释放。

SDK oracle 核验 STARTUPINFOEXW x64 112/x86 72、attribute offset 104/68、flag/Job attribute 与 WINAPI prototypes。mock 覆盖次序、stable value、query size/错误、init/update/create 清理及 resume/stop 失败。Windows native pipe/进度监督、worker 对自身启动契约验证、Hook 存活和正式可执行 host 仍需接入；生产 host 必须在打开输入前完成启动核验。

依据：[Microsoft Job 属性说明](https://github.com/MicrosoftDocs/sdk-api/blob/docs/sdk-api-src/content/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute.md)、[InitializeProcThreadAttributeList](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-initializeprocthreadattributelist)、[STARTUPINFOEXW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-startupinfoexw)、[DeleteProcThreadAttributeList](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-deleteprocthreadattributelist)。
