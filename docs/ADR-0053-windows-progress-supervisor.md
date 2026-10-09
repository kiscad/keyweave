# ADR-0053：Windows 独立进度 Supervisor

父进程独占自己创建的 detached Job worker、两条 owned overlapped pipe 父端和共享 ProgressWatchdog。spawn 自动增加固定 worker 标记、创建父 PID 和正 run ID 参数，在明确可信的 absolute executable 上执行；配置参数为独立 argv，不经 shell。先验证 policy 与有界完整命令行（固定前缀占3个参数，额外参数最多125个；原参数长度检查在任何 OsString clone 前完成）、创建 private pairs，再原子 Job/精确 HANDLE_LIST 启动，最后 resume。Job/启动错误保留原 owner；resume 错误返回 Supervisor owner、主错误，仍需显式停止及轮询退出。不开放按 PID 接管、raw handle、Clone 或跨线程移动。

poll 在 callback 外、一次零超时 process wait、至多一次 challenge Write 与一次 receipt Read。时间由同 epoch MonoTime 显式提供，先核验 lease 再收 buffered frame；WouldBlock/partial 不续租，request shadow 在 flight 中不改变。仅在 challenge 的完整 Write ack 后启动 receipt Read：overlapped 写操作返回 WouldBlock 后，child 可能已读到数据并开始回复，提前读不能被解释为 child 违规。queued receipt 在 Write ack 前不读取、不续租，完整回应随后才被校验；共享 codec/watchdog 验证 run、单在途 id、baseline/epoch、phase 和边界。新的 challenge 在下一 poll 才发送。Stopped receipt/EOF 不是退出证明：保留有限最后 lease，等待真实 signaled exit，逾期仍停止。半帧 EOF、提前 EOF、非法计数、IO/协议/时钟错误失败关闭。

第一停止原因锁存；停止失败保留 pending stop 并返回 API 错误，后续 poll 重试，不再读写/续租。process wait/query 错误也锁存并尝试停止，同时保留主错误和停止错误。TerminateJobObject 成功只表示 stop_sent；Exited 只来自 owned process 的真实 wait/query（259也是合法退出码）。仅观察 Exited 后允许显式关闭父端，reader/writer 各自报告所有取消/关闭结果。Drop 只沿用有界 Job/pipe fallback，不声明已退出或资源全清，不清理未知的全局注入键。

生产 owner 必须独立于 worker 捕获循环，及时驱动 poll（建议最多20ms housekeeping，最近 deadline 更早时优先），不得以后台心跳或 worker 缓存计数续租。组件不包含完整 host/捕获 worker 可执行入口；无输入 fixture 可证明真实进程/transport/停滞终止，不证明 CaptureSession 实际 poll、Hook健康、桌面输入恢复或Win11覆盖。共享协议不复制，Linux Supervisor 暂保留自身已验收适配器。

异步完成依据：[GetOverlappedResult](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresult)、[Synchronous and Overlapped Pipe I/O](https://learn.microsoft.com/en-us/windows/win32/ipc/synchronous-and-overlapped-input-and-output)。快速回应先于父端收集 Write ack 的时序是基于独立进程调度与异步结果收集的推断；不假设跨两个 handle 的完成观测顺序。专门回归将reply预先queued而Write暂时WouldBlock，要求不读/不续租，ack后才接受；旧逻辑误报Identity，恢复最终源码后全套重验。
