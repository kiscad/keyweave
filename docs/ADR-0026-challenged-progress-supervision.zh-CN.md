# ADR-0026：请求编号约束的事件循环进度监督

状态：采用。关联 D06。尚不代表实际键盘 worker/OS 循环已可运行。

## 问题与选择

仅看后台线程 alive 或收到逐增计数都不足够：IPC 可能积压在真正卡死之前发出的进度。采用单在途 challenge：监督者发出 run_id/challenge_id；worker 接收时保存 Runtime.progress_epoch baseline，之后至少完成一次真实 Runtime.poll，才可回传 baseline/新 epoch/phase。新 epoch 必须大于 baseline 与最后接受值，且请求编号匹配。每次只应答一次，不能刷新旧 challenge。所有帧均有固定 magic/version/reserved，固定 24-byte 请求和 40-byte 响应，不携带按键、配置、窗口标题或用户文本。

ProgressClient 收到 challenge 时只读 Runtime 进度；publish 只读 Runtime 产生的 epoch/phase，不能通过调用 publish/flush 自行递增。每次读写一次，partial frame 保留，WouldBlock 不循环，队列只有一个请求/响应。后台线程重复读取冻结进度不能满足 baseline 的要求。协作式协议不证明恶意 worker 没有手写伪造响应；可信 worker binary 与唯一父子通道是权限边界。

ProgressWatchdog 使用监督者注入的单调时间。默认启动期限 10s、进度租约 2s；`>=` 期限拒绝迟到响应，即使缓冲里有旧帧也不能复活。phase 只允许 Active→Recovering→Stopped；Stopped 仍需实际进程退出，不能仅凭声明清设备状态。clock/格式/run/request/epoch 错误进入明确终止原因。

Linux Supervisor 启动并独占自己创建的 Child；用两条匿名 pipe 接 stdio stdin/stderr 作为二进制控制通道，stdout 由调用者配置。四个端点通过私有 fcntl 边界设非阻塞（保留原 flags），无需网络或 FD 副本；双方非阻塞，监督者每 poll 只做一次 request write 和 response read，先检查 try_wait 与期限。父进程不打开或继承键盘 grab FD，worker 独占所有输入/uinput；不能按任意 PID 发信号。期限/协议/通道失败发送 Child.kill，随后继续 try_wait；只有真实 exit status 才返回 Exited 并证明已收割。KillRequested 与 Exited 独立。

spawn 接收受信任的固定 worker Command，不是用户配置的 shell/action 执行入口。调用者必须在启动工作进程前完成配置/权限约束，worker 在 watchdog 已建立之后再打开设备。控制帧占用 stderr，worker 不得混入文本日志；诊断另走有界状态接口。

Drop 为异常路径发送 kill 并做短期有限 try_wait，不能无期限 wait。正常宿主必须持有 Supervisor 并 poll 到 Exited；内核不可中断阻塞、kill/try_wait 错误可能使退出/FD 释放尚未证实。Rust Child 本身不会在 Drop 时终止或自动 wait，依据 [Child 官方文档](https://doc.rust-lang.org/std/process/struct.Child.html)。此处只使用本机 Rust 1.97.1 已稳定 API，不采用当前文档的 nightly pidfd/group 扩展。

## 尚未完成

Supervisor 组件可管理无设备 worker 的真实进程，但未接入生产 CLI worker、OS poll loop、parent-death guard、启动 neutral/consumer-ready 事务与实际 hardware fault tests。监督者死亡后的 worker 自动退出须另实现；当前不得发布为无人值守 service。启动测试子进程不访问输入设备或加载驱动。

本机 IPC 路径选择：初版 UnixStream 的 send 在执行 sandbox 返回 EPERM，未通过子进程协议测试。改用 Rust 1.97.1 已支持的 [std::io::pipe](https://doc.rust-lang.org/std/io/fn.pipe.html) 与非阻塞文件 flags，不升级 sandbox 或要求网络。Command 在 spawn 后立即 drop，父端不能保留阻碍 EOF 的 worker endpoint。该调整不改变 challenge 协议。

## 本次验证结果

协议六项行为测试验证字节布局/拒绝、启动和租约 `>=` 边界、旧请求/冻结计数/错误 run、不接受 phase 回退及 Stopped 未退出、单调时钟/溢出。三项真实 Runtime 客户端测试证明 challenge 到达前的 epoch 不够、partial/WouldBlock 不改变原 receipt、后续请求取新 baseline、错误 run/跳过编号不写响应。四项 Linux 子进程行为测试覆盖启动无进度、后台线程 alive 而事件循环停滞、错误 run、成功退出/主动停止；有一个复用 libtest worker fixture 的入口，无外部 shell/no keyboard，所有 owned children 在正常测试路径 try_wait 收割。workspace 共 327 个通过；fmt/clippy/source hash/本机 UAPI ABI probe 通过。
