# ADR-0092：单任务冷配置准备与实际 actor 交付

日期：2026-10-11。状态：portable 冷任务与交付桥已实现；原生 worker 控制循环尚未接入运行中 reload。

## 生命周期与资源边界

`keyweave-worker::reload_preparation::PreparationSlot` 是控制线程持有的非 Clone owner。每个 owner 最多一个后台任务或一个已完成结果；Ready 未交付也返回 Busy，不覆盖已有候选，不启动并行编译。start 在创建线程前检查 outside-callback、严格 Request 与实际正文长度；正文来自 ADR-0091 的 Receiver.take，最多 8MiB。Linux Target 持有冷输出 DeviceSpec，Windows Target 采用 Hook capabilities。

后台线程使用已有两平台 PreparedConfig::from_bytes_for_generation，将 Request.target 交给编译器；UTF8、无损编译、引擎与平台输出准入保持不变。原始正文、PreparedConfig 和冷诊断在后台销毁；回传仅保留严格 Request、平台标记以及不可变 Arc IR，或编译失败标记。编译失败产生 Rejected，而非进程内 panic 或部分配置激活。任务 panic 由 join 识别并永久关闭 slot；不修改系统 panic hook，不保证 panic hook 的第三方日志行为。

poll 先检查 outside-callback，仅在 JoinHandle.is_finished 后消费原 handle 并 join；运行中的任务不会 join、sleep、等待 channel 或被重新启动。close 永久锁存 Closed 并丢弃已完成结果，运行中的 job 保留在原 owner；后续 poll 回收它并丢弃晚到结果。close 不抢占编译，不生成 Cancelled/Accepted ack，也不制造 progress/epoch。停止时调用方同时关闭 Receiver/lane。

原生控制循环必须在关闭输入并完成恢复期间保留、驱动同一 slot。Drop 本身不等待后台编译，未完成线程会 detach；这是显式限制，不是终止任务的证明。每个 owner 的数量有界，不证明调用方能无界创建新 owner 仍然有界。解析、正则、分配、线程创建、已完成 join 的收尾与调度没有硬实时上界；不得在输入回调创建线程/解析正文/调用 poll 或 deliver。

## 激活先于确认

Receiver 新增纯 awaiting_commit(Request) 查询，只有未关闭、已 take、未 finish 的精确事务才成立。deliver 在任何 actor 变更之前核对这个状态和 actor.config_generation()==Request.base。错误 receiver、已关闭 receiver、过时实际代号永久关闭 slot 与 receiver，保持 actor 原状态，不把旧候选应用到新会话。

ReloadActor 是 sealed trait，仅实现于共享 Engine、Linux Runtime<S,B> 和 Windows SessionActor<B>；外部不能用一个返回虚假代号的 closure 伪装激活。平台 actor 只接受本平台 Target；Engine 用于可确定性回放，接受两平台已冷校验的快照。deliver 核验 IR 的 target 代号，再执行实际 actor.activate；原 actor 的 active/stop、变量历史、freshness 与输出校验仍是最终准入。失败返回 Rejected，成功返回 Accepted，随后 Receiver.finish 使用实际 actor 的当前代号构造 ack。

此方法不写 ack、不更新 progress、不续租、不做输入/输出系统调用。Receiver 仍停在 AwaitingAck，必须用 ADR-0091 的 AckWriter 与精确 AckSent 才能重开事务。commit 后 ack 失败不能 rollback；父端可能不知道实际已激活，只能停机并重新建立独立会话。Request 匹配不是认证或内容哈希；可信 owner 与独立端点绑定仍是前提。

## 下一步接入顺序

1. 建立 owned、nonblocking、独立的原生 reload 端点；不得复用 progress/Stop 流。固定 header/body 每步 IO 预算保持不变。
2. 在原 worker 控制循环处理 Stop/访问撤销/parent lease/session readiness/time 之后，才推进 Receiver；Ready.take 后交付唯一 slot，输入与定时工作继续由原 actor 执行，不因任务等待而制造心跳。
3. 同一控制线程 poll 原 slot，并在 deliver 前重新验证全部会话与独立时间前提。Windows CaptureSession 还需沿用 outside-callback 与 try_borrow/try_borrow_mut 原生封装，不能绕开其独占 actor 合约；当前 trait 实现本身没有取得 CaptureSession 内部 actor。
4. 实际激活后驱动 AckWriter，每次 ack IO 前后继续独立 lease/time/Stop 检查；失效先关闭捕获并恢复，不发迟到成功确认。失败保留已实际激活代号，不自动重发正文或回滚。
5. 最后接 host 文件监听与 reload 命令；debounce/读取/编译不能成为父监督续租依据。快照刷新只能来自 actor 实际 Accepted ack，文件路径或准备完成不是证明。

## 验证与限制

Linux Rust 1.97.1 离线 locked workspace/all-targets 904 项测试、Clippy -D warnings、fmt、60 项固定来源哈希校验通过。新增 9 项 portable 测试：两平台工厂→真实 Engine→AckWriter token，坏配置拒绝，Ready/单次交付，错误/关闭 receiver，过时代号，Stop/长度先验，真实共享 Windows SessionActor 的成功/Stopped/平台拒绝且零输出，阻塞中的同一 job 停止后回收，panic 关闭。任务等待测试只驱动原任务，5 秒观测预算不重启任务。

未运行原生 Windows 双架构补验；此前自动审批额度错误仍未解决。新增 Linux Runtime trait 已编译但未构造真实设备；Windows actor 测试在 Linux 使用共享 actor 与禁止输出的 mock backend，不是 Windows Hook/SendInput 或真实桌面验收。本轮没有文件/原生输入/输出/挂起/driver 操作，未增加 unsafe/FFI/外部依赖；将已有 workspace-local keyweave-progress 从 dev dependency 提升为 production dependency，Cargo.lock 未变化。原生 IPC、实际控制循环、watcher、reload CLI 和完整移植目标继续未完成。

见 [验证记录](validation/linux-2026-10-11-reload-preparation.json)。
