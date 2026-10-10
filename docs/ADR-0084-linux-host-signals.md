# ADR-0084：Linux host 信号接收与 child 掩码恢复

日期：2026-10-10。状态：组件已实现；host 生命周期和 CLI 待连接。

## 决策

用 `HostSignals` 在 sole main thread、其他线程与 worker 创建之前阻塞 SIGINT/SIGTERM/SIGHUP，以原子 NONBLOCK/CLOEXEC 创建 signalfd。拒绝已有 managed mask 或非默认 disposition、非主线程、已有多线程或重复 owner。不安装异步 handler，不在后台造进度。每次 poll 最多读一个 128-byte native record；WouldBlock/EINTR 只表示未消费事件，不证明 worker 存活。wait 限制 20ms，readiness 不是事件消费。

该接口遵循 [signalfd](https://man7.org/linux/man-pages/man2/signalfd.2.html) 与 [线程信号掩码](https://www.man7.org/linux/man-pages/man3/pthread_sigmask.3.html) 的契约。信号掩码是每线程状态，并跨 fork/exec 继承；后续线程须继承阻塞掩码，不能自行解除。`/proc/self/task` 是有限快照，不能替代调用方禁止并发创建线程的责任；不承诺通用多线程 embedding。

## child 与关闭顺序

`prepare_child` 验证当前 managed mask 仍阻塞，在可信 Command 的 pre_exec 中，仅用 async-signal-safe `sigprocmask` 恢复安装前的准确掩码。不改 host 掩码，不改 disposition，不读取配置，不获取父锁，不执行用户命令。前序独立组启动仍保留 session 与父死亡保护。CLOEXEC 防止 signalfd 泄漏至 worker。调用方后续 pre_exec 不得重绑 group/session/mask 或选定 FD。

控制台 `verify_isolated_group` 现在额外只读检查三个 stop signals 均未阻塞，拒绝遗漏恢复的 child。None 无设备 invariant 不变。

owner 不可 Send/Sync。显式 close 应在同一个线程、owned worker 实际退出和清理后执行，先关闭 FD，再恢复原 mask。恢复可能使并发到达的 pending signal 按原默认 disposition 立即交付；这不是吞掉所有后续信号的机制。Drop 只做同序 best effort；失败不能被称为已完成恢复或实际 exit。

## 验证

见 [验证记录](validation/linux-2026-10-10-host-signals.json)。新增纯 record 校验与非 main-thread 拒绝测试；实际 main 无输入 harness 安装 guard，在独立测试进程自身 PID 上发 INT/TERM/HUP，逐个消费并继续存活。绝不向终端前台组或用户进程发送信号。

同 harness 的正常 isolated sealed-config child 校验 unblocked mask、独立组与同 session，扫描自身 FD 证明无 signalfd。故意漏掉 `prepare_child` 的 child 被 guard 拒绝。关闭 owner 后再次安装成功，验证原 mask 已恢复；三协议管道、sealed 字节、真实 exit 与前序父死亡测试保持通过。C oracle 只检查 native ABI/prototypes，不发信号、不访问输入。

没有真实终端 Ctrl+C、VT、evdev/uinput、Hook/SendInput 或驱动验收。Windows 前序原生补验仍待完成。

## 下一步

把接收事件锁存为一次协作 Stop，请求不能延长 progress lease 或 shutdown deadline；继续 poll 同一个 Supervisor 到实际 exit，之后才有界收集 KWLT。分别保留 signal、主故障、超时 kill、actual exit 与 cleanup/report facts。随后连接 cold host preparation 与 run-linux CLI。收到信号本身不证明 Stop 送达、实际退出或输入恢复，完整跨平台目标尚未完成。
