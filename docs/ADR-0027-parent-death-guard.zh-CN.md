# ADR-0027：Linux worker 父线程死亡保护

## 问题与选择

进度租约能发现 worker 卡死，但 supervisor 被终止后不能继续主动 kill。worker 必须在打开设备之前获得内核保护。Linux `PR_SET_PDEATHSIG` 绑定创建子进程的父线程；若只在 exec 后安装，父线程可能已经退出，`getppid` 仍可能指向存活的父进程，无法发现该线程的死亡。

Supervisor 在 Command 的最后一个 pre_exec hook 中安装 SIGKILL，前后核对预计父 PID。spawn 的 exec 握手返回之前，创建线程仍在调用 spawn；因此正常返回已越过安装空窗。父进程提前死亡导致 PID 改变时，hook 返回 ECANCELED，禁止执行 worker。hook 仅使用 getppid、直接 prctl syscall 和无分配的 raw errno；不读取环境、不分配、不加锁、不记录日志。

Supervisor 写入预计父 PID 环境变量。worker 主线程在任何设备打开之前调用 WorkerParentGuard::from_supervisor_environment，核对 PID、线程身份与 PR_GET_PDEATHSIG==SIGKILL，并再次核对父 PID。核验失败立即结束启动。该 token 只是启动时核验，不能阻止后续代码自行清除保护。

## 启动与生命周期约束

- spawn 必须由贯穿 worker 生命周期的控制线程调用，不能借短命线程启动后退出。线程退出会杀掉 worker，即使父进程其他线程仍在运行。
- Command 必须为可信直接 worker；不得经 shell、sudo 或包装器替代。调用者已有 pre_exec hook 必须满足 Rust 的 fork 安全要求；本 hook 最后注册。禁止其他代码随后清除保护。
- setuid/setgid/file capabilities 的 exec 可能清除保护，主线程核验会拒绝；不自动重装以掩盖启动缺口。权限应在可信启动方案中确定。验证后不得改变 effective/fs uid/gid 或 exec/fork 并继承设备；需要此能力时另作受监督设计。
- 子线程不会成为主线程保护的替代持有者；核验 API 拒绝在非主线程调用。token 不可 Send/Sync。
- SIGKILL 不能被 worker 屏蔽/处理，但内核不可中断任务仍不保证即时退出。只有 actual exit status 才能证明退出；硬件释放仍需真机验收。
- 不按外部 PID 发信号，不在测试运行器中安装信号，仅在自己 spawn 的子进程的 pre_exec 安装。

## 验证与替代

离线子进程测试核验主线程保护、错误父 PID 与错误线程拒绝，以及创建线程退出后由内核 SIGKILL、原 Child 被收割。测试不打开键盘/uinput。完整 supervisor 进程崩溃及持设备故障注入仍属于后续真机验收。

只依赖 pipe EOF 无法结束已经卡死的 worker；只依赖 supervisor Drop 无法处理 SIGKILL；exec 后设置信号无法关闭父线程死亡空窗。pidfd 可用于后续身份管理，但不替代这里的内核死亡信号。

依据：[Linux PR_SET_PDEATHSIG](https://man7.org/linux/man-pages/man2/PR_SET_PDEATHSIG.2const.html)、[PR_GET_PDEATHSIG](https://man7.org/linux/man-pages/man2/PR_GET_PDEATHSIG.2const.html)、[Rust CommandExt::pre_exec](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.pre_exec)。直接 syscall 的 fork 后无锁路径参考 [glibc x86_64 syscall 实现](https://github.com/bminor/glibc/blob/master/sysdeps/unix/sysv/linux/x86_64/syscall.S)，未复制第三方实现。
