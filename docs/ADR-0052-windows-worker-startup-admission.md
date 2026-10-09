# ADR-0052：Windows worker 启动准入与 private async 端点

WorkerStartup 必须在真实 main、创建其他执行线程和打开输入/安装 Hook 前调用，失败后退出而不重试。一次性 claim 先检查 callback，再全进程原子锁存；核验正 parent/run ID、Toolhelp snapshot 中实际创建父 PID、GetStartupInfo 与当前标准句柄绑定、立即 Job 的 kill-on-close/active=1 限制及唯一当前 PID。查询 Job 使用 NULL，绝不打开/继承 Job handle。token 为 !Send/!Sync，不能证明之后的外部代码未改变进程状态，也不替代受信 host 的原子 Job 启动契约或跨用户鉴权。

stdin/server reader 与 stderr/client writer 必须是两个不同的本地单实例、拒绝远程的 byte/WAIT KeyWeave pipes；两端的 creator/peer PID 必须等于实际父 PID。只读取 scalar、私有随机 pipe 名与必要权限，不读账号/窗口/按键内容，不导出 pipe 名。stdout 必须是 char 类型（host 提供 NUL）。NtQueryInformationFile 的 FileModeInformation(16) 必须为本工厂的零 mode，FileAccessInformation(8) 核验 read/write 方向与属性读取权限，FileNameInformation(9) 只接受固定前缀与32个小写hex字符并比较两管道 identity。metadata-only Win32 pipe state 不能替代 async mode 核验。

Native query 使用 stable heap/UnsafeCell 输出；仅 STATUS_SUCCESS 且最终 IO_STATUS_BLOCK 成功、长度合法时读取。任何非零返回（包括 STATUS_PENDING）拒绝准入，保留输出内存至进程退出，不等待/重用/读取可能仍被 OS 写的 buffer。一次性失败锁存禁止续用迟到结果。调用仍可能受 OS 调度/元数据锁影响，不是实时保证；它只在输入启动前、callback 外执行。

核验后清除三个原标准 handle 的继承位，只 DuplicateHandle 两个进度 pipe 为 noninheritable private copies。复制后拥有独立 manual event/OVERLAPPED/buffer，沿用有界 transport；不接管/关闭原 std handle、不复制 Job/device handle。stdin/stderr 保留至 process exit，禁止同步 std::io 读写或日志混入二进制协议。private client Drop 不证明原端点 EOF。构造故障保留主错误并用既有有限资源 fallback；失败路径要求 process exit，不声明 rollback 已全部完成。

SDK oracle 核验 Toolhelp/IO_STATUS_BLOCK/标准句柄布局与 SDK 支持的 prototypes/constants。SDK winternl.h 只公开部分 FILE_INFORMATION_CLASS，未声明 NtQueryInformationFile；class8/9/16、ULONG 信息体与 Native API 签名依据官方 DDI 文档，不能宣称由该 SDK 的 NtQuery prototype/WDK oracle独立证明。x64/x86 实际链接/同步拒绝/异步接收/跨进程协议验收另记。

依据：[NtQueryInformationFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntqueryinformationfile)、[FILE_MODE_INFORMATION](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_mode_information)、[QueryInformationJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject)、[GetStartupInfoW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getstartupinfow)。

启动标志改为 `DETACHED_PROCESS | CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT`（0x8000c）。首轮真实 child 在 `CREATE_NO_WINDOW` 下，Job PID 列表需要两项；加大临时诊断 buffer 得到 assigned=count=2，其中包含当前 child。改用 detached 后同一无输入 fixture 得到 assigned=count=1，仅当前 PID。附加进程与无窗口控制台创建相关是推断，未读取该进程身份；不按额外 PID 操作、不把扩大名单或忽略计数作为修复。正式准入仍只接受一个 PID，不创建/继承/分配 console；显式 STARTF_USESTDHANDLES 的 pipes/NUL 独立保留。普通 argv/stop、端点交付、父死亡 fixture 必须在最终标志下重验。[Process Creation Flags](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)、[Creation of a Console](https://learn.microsoft.com/en-us/windows/console/creation-of-a-console)。
