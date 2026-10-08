# ADR-0029：独立 worker 的继承管道接收

## 问题与选择

Supervisor 已把两个非阻塞管道分别映射到 worker stdin/stderr，并设置父身份与 run ID；run_worker 需要拥有无缓冲的 Read/Write。直接用 File::from_raw_fd(0/2) 会擅自取得标准流所有权，与 Rust 标准 IO 及后续 FD 复用冲突。

新增 WorkerStartup：真实主线程先核验 WorkerParentGuard，再严格解析正的 u64 run ID；查询两个借用的标准流 FD，要求 S_IFIFO、读/写方向匹配、O_NONBLOCK 且 dev/inode 不同。全部通过之后，通过标准库 try_clone_to_owned 取得两个独占、close-on-exec 的 File，只复制进度管道，绝不复制 evdev/uinput。构建 ProgressClient<File,File>，留待启动事务与 run_worker 消费。

这里验证的是管道结构与启动约束，不是跨用户鉴权；父进程必须是固定可信 supervisor。FIFO 类型不证明是匿名管道，生产来源由 Supervisor 保证。普通终端、文件、同一管道回环、反向或阻塞端点全部拒绝。任何错误发生在设备打开之前。

## 所有权约束

WorkerStartup 在每个进程中只接收一次。主线程静态 AtomicBool 仅用于启动端点 claim，不持有引擎/按键状态；claim 以后失败不可重试，worker 应退出。guard/run ID/端点结构验证失败不改变端点或 claim 状态。复制/创建失败由 RAII 清理已取得的 FD。

原 stdin/stderr 保留，避免关闭标准流后 FD 被错误复用。worker 生命周期内不得用标准输入读取命令、向 stderr 写日志、额外复制或转交进度通道。client Drop 只关闭其 copies；原端点直到进程退出才关闭，因此 pipe EOF 不能作为 worker 退出证明，Supervisor 继续使用 Child::try_wait。进度不包含按键数据，panic/误写会导致协议拒绝与终止。

WorkerStartup 包含 guard、run ID 和 client，不可 Clone/Send/Sync；into_parts 消费它，用于启动流程或运行循环。构造成功不等于输出就绪，不抓键盘，也不伪造 Runtime epoch 心跳。可执行 worker 与多设备/consumer-ready 启动事务仍需随后接入。

## 验证

真实管道测试覆盖方向、阻塞/非管道拒绝、回环拒绝、CLOEXEC 和 owned copies 的实际读写/Drop。独立 main-thread harness 覆盖 Supervisor 注入后接收、run ID 错误、错误线程、重复接收与 Drop 后禁止再次接收，不访问输入设备。原始协议 fixture 单独读取/响应以验证进程链路，实际 client I/O 仍由既有 Runtime client 测试和 owned pipe 读写测试覆盖；不把它描述为生产 worker 完整启动验收。

依据：[fcntl duplication](https://man7.org/linux/man-pages/man2/F_DUPFD.2const.html)、[fstat](https://man7.org/linux/man-pages/man2/fstat.2.html)、[Rust FromRawFd 所有权要求](https://doc.rust-lang.org/std/os/fd/trait.FromRawFd.html)。本实现不新增 from_raw_fd；采用标准库拥有的 FD 转换。
