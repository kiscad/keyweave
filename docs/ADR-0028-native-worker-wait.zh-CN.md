# ADR-0028：生产 worker 的单调时钟与事件等待

## 选择

用 CLOCK_MONOTONIC 的绝对纳秒作为 Runtime 时钟，与已设置 EVIOCSCLOCKID 的输入保持同一时钟域。reactor 使用有限 ppoll，取 runtime deadline 与 housekeeping 上限的较小值；不使用固定 sleep 模拟输入事件等待，也不依赖墙钟。

wait set 最多 67 项（64 输入、1 输出、2 进度管道），同步借用 owned FD，不复制、关闭或向外导出设备 FD。捕获设备/输出设备仅增加 crate 内部 BorrowedFd 访问，生命周期不能超过 owner。进度 reader 只在可以接收新请求时关注 POLLIN；writer 只在已有待发送 response 时关注 POLLOUT，避免永远可写导致忙循环。输出设备关注故障，已有输出仍由 Runtime 的 deadline 安排；LED 反馈接入另做。

每步顺序是接收 challenge→实际 Runtime poll→发布 response→按最新 deadline 等待。等待返回后重新取时钟，不把原 deadline 伪装为实际时间。EINTR 交还外层，不内部无限重试；HUP/ERR/NVAL 保留，不能将其解释为普通超时。输入/输出故障撤销全部 capture，进度通道故障进入控制恢复；任何外层错误用当前最后可信时钟暂停并执行最多已有恢复预算，随后 Drop 所有资源。

worker loop 接收已准入的 Runtime、owned 非阻塞进度端点和主线程 WorkerParentGuard。该入口接通生产 OS 等待与执行，不替代多设备启动事务、桌面消费者 ready、权限/会话管理和 CLI。它不自行发现或抓取设备。本次测试只使用匿名管道、内核时钟及 mock Runtime，实际键盘/uinput 尚未验收。

## 约束与证据

默认 housekeeping 20ms，单次等待 API 最大 1s；deadline 已到时不等待。内核调度可能超过请求等待时长，不能承诺硬实时。保持单线程引擎与原有 bounded poll、challenge epoch 语义。生产入口不得改用后台线程单独续约。

依据：[Linux ppoll](https://man7.org/linux/man-pages/man2/poll.2.html)、[CLOCK_MONOTONIC](https://man7.org/linux/man-pages/man2/clock_gettime.2.html)。FFI 留在 ioctl.rs 并逐处解释内存/借用/ABI，不引入其他 unsafe 模块。
