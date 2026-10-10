# ADR-0083：Linux 控制台 worker 独立进程组

日期：2026-10-10。状态：已实现组件；完整 host 信号事务与真机验收待完成。

## 问题与决策

终端产生的 SIGINT 发给前台进程组。若 host 与 worker 同组，worker 可能在 host 发送 Stop 前被终止，绕过协作恢复。因此显式控制台启动 API `Supervisor::spawn_reporting_config_isolated` 使用 `CommandExt::process_group(0)`，把 child 置于以自身 PID 为 PGID 的新组。保留 session 和控制终端，使 ConsoleProvider 能继续要求父组是选定 VT 的前台组。

不调用 setsid，不调整 host 组，不安装 host signal handler。本组件是后续 host 协作停机的前置条件。原 spawn API 保持调用方的 process-group 设置，移除继承的隔离 marker；显式模式覆盖 marker 为 `1`。契约基于 [Rust CommandExt](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html) 和 [Linux 进程组与会话接口](https://www.man7.org/linux/man-pages/man2/getpgrp.2.html)。

## 校验顺序

`WorkerParentGuard::verify_isolated_group` 先重新校验父死亡 SIGKILL 与父 PID，再要求模式 marker 精确为 `1`；随后只读查询 PID、自身 PGID、父 PGID、自身 SID 与父 SID。自身必须为组首、父组必须不同且有效、双方必须处于同一个有效 session，最后再次校验父 PID。

marker 只是选择契约，不构成身份认证；元数据查询不是原子快照，也不排除 ABA。当前组状态不能证明未来不会被外部改变或直接收到信号。

真实 main 的 console 分支在 claim/read 配置前执行该检查，失败返回 Admission=65。provider 的打开与每次观察、bootstrap 的构造与每步、活动 console worker 的准入都要求同一契约。None 分支仍永久不打开输入设备。console bootstrap mock seam 继续使用注入的验证行为。

可信 Command 调用方的 pre_exec 不得改变组/session 或关闭、重绑选定配置 FD。创建线程必须活到实际 child exit；保留原父死亡保护。不要把独立组误认为 signal blocking 或 signal ignoring；直接对 worker PID/其组发信号仍可能终止进程。

## 验证与边界

完整离线测试、Clippy、格式、60 项上游来源哈希及 C ABI 原型检查通过，准确计数和源码/日志哈希见 [独立验证记录](validation/linux-2026-10-10-isolated-group.json)。新增证据包括：

- 无输入实际 main 子进程校验独立组、同 session、父死亡 guard、三条私有协议管道、密封配置字节与实际退出；旧模式同时验证 marker 被移除。
- 创建线程退出后直接 reap 同一个 owned child，确认内核 SIGKILL，而非 Supervisor 的故障 kill fallback；独立组保留原保护。
- 纯组形状校验拒绝继承组、相同父组、异 session 和无效标量。
- 真实 console exe 使用旧 spawn 和无效配置，先返回 65，证明未进入配置或设备操作；严禁把此 fixture 改成隔离模式后继续执行真实 VT 查询。

没有操作真实 VT、evdev/uinput、Hook/SendInput、驱动，也没有发送前台组信号或模拟实际终端 Ctrl+C。Windows 前序待补原生检查仍未执行；本轮 Linux 专属实现不能替代 Windows 证据。

## 后续工作

实现 host 有界 signal/control 接收与单一 Supervisor 生命周期：停机信号转换为一次 Stop 请求，实际 exit 后才观察 KWLT，分别报告正常协作、超时 kill 和不完整恢复。先验证 own child 与可注入控制事件，再实现 run-linux CLI；保持 None 测试无设备。实际控制台、暂停/切换、Ctrl+C、硬件释放与系统权限验收另行执行。完整跨平台开发目标尚未完成。
