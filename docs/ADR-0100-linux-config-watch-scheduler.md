# ADR-0100：配置读取调度与确认基线

日期：2026-10-11。状态：Linux host 库调度器与无输入组合测试已实现；用户命令入口尚未接入。基于 ADR-0097～0099，不改变 worker 的配置准备、实际激活或确认协议。

## API 与接入要求

`linux::watcher::ConfigWatcher::new` 冷构造，要求绝对 OS 路径、初始封存 generation 1 的精确原始正文，以及100ms～60s检查间隔；构造不访问文件、不建线程。首次 poll 要求原 NativeSession 提供 reload 通道且 observed generation 仍为1，绑定该 run。调用者必须独占该 session 的 reload 提交与结果，不得另行 request_reload/take_reload_outcome，初始正文正确性仍由可信调用者负责。run ID 是现有会话关联信息，不是恶意调用方隔离或认证。

每次 poll 先驱动同一个 NativeSession，保留原 progress、Stop、watchdog、同一时钟和实际 Exit 观察；只有 Pending(Running) 才处理文件状态。已绑定的错误 session 在驱动它之前被拒绝，关闭 watcher，不能因此停止新的无关 worker。关闭后的 poll 仍驱动原 session；错误返回也要求调用方继续原 owner 的停止/退出监督，不得从 watcher error 或 Drop 推断退出。

首次 Active/Running 且 reload lane 空闲后启动文件任务；此时 HostSignals 安装和原 worker spawn 已完成。后续采用固定间隔**轮询正文**，没有 inotify/debounce 或任意 async executor。now 使用原 pump 最后接受时间，仅作为文件调度提示，不授权新会话租约、不延长 reload budget。`request_read` 仅设置一个合并标志，忙时不增线程/候选/事务或延长期限；它仍按正文比较，不能强迫重复激活同一正文。

`take_event` 消费单个状态通知：Unchanged、Submitted、Accepted、Rejected、Cancelled、ReadFailed、Closed。Submitted 表示队列准入，Accepted 才是原协议的有效确认；通知槽不无限积累，确认可覆盖尚未消费的 Submitted。未消费最终通知会暂停后续文件读取，但原 NativeSession 每 poll 仍继续监督，确认处理也不被 Submitted 阻挡。库不在活动循环输出日志、配置正文、路径或 run ID。

## 单任务比较与基线

FileSource 增加 request_compare(seen, active)：两个不可变 Arc 引用与原文件任务一起转交后台线程，读取规则沿用 ADR-0099。精确原始正文比较、changed 候选的保留副本创建也在后台执行，控制线程只移动 Box/Arc，不同步扫描8MiB正文或重新解析。

seen 是最近提交正文，active 是已确认激活正文：

- 与 seen 一致返回 Seen，不发送事务。Rejected/Cancelled 后重复相同坏文件受到抑制。
- 与 active 一致返回 Active，将 seen 恢复为 active；恢复已经激活的文件无需额外 generation。
- 其他正文返回 Changed，保留一个 owned 传输正文及一个 immutable baseline 候选；等待实际 Active/空闲准入后提交，随后停止下一次文件任务直到确认与通知消费。

只有对应 Accepted 且原 lane observed generation 等于 base+1 才将 active 更新为候选；Rejected/Cancelled 要求 observed generation 保持 base，旧 active 不变。无法确认/丢 ack 不自动重发或 rollback，可能已在 worker 激活的事实仍按 ADR-0097 处理。正常文件 IO、空/超限或 Changed 错误仅发布 ReadFailed，下一次间隔或手动请求可再读，旧配置保留。任务 panic、spawn/内部状态错误、时间倒退、代号错误或提交失败关闭 watcher，并对同一个已准入 owner 记录 Control fault/执行原停止路径；无重启替代线程。

每 owner 同时最多一个文件任务/Ready、一个候选或在途正文及一个通知标志；active 与 seen 可能分别保留不同8MiB正文，changed 还需传输正文与候选 Arc。此为正文/任务数量限制，**不是8MiB总RSS或硬实时保证**；allocator、析构、线程调度和普通/网络 filesystem IO 仍无硬时间上界。公共 FileSource 对可信调用方提供的 Arc 不作全局 owner/内存数量限制。

## 停止与回收

原 NativeSession Stop/故障/退出先关闭 reload lane；watcher 随后永久关闭 source、释放候选和在途 baseline，晚文件结果不能提交。保留同一个 FileSource handle，通过 reap_closed 非阻塞回收完成任务；没有超时后启动新线程的路径。运行中线程不能强制取消，Drop 可能 detach，不证明回收完成。

单独调用 watcher.close 只关闭文件调度，不撤回已在 NativeSession 排队或传输的请求，worker 仍可能实际激活它。其结果必须按原 NativeSession 事实继续观察；不能把 close 当事务 Cancelled、旧配置仍激活或 child Exit 的证明。用户 Stop 应继续调用原会话监督，直到实际 Exited，独立审查终止事实。

## 验证与后续

新增9项调度单元测试和1项自身临时文件后台比较测试：周期/Active/通知门控、Accepted越过Submitted通知、手动请求合并、Rejected抑制/恢复active、Cancelled独立通知、正常读取错误恢复、Stop丢候选/原任务回收、错误run零驱动/零停止、初始baseline、倒退/错误代号、fatal source及冷构造。单元 Peer 是模拟对象，不能用作 native lease 或设备就绪证明。

原 actual-main linux_reload harness 增加第三 watcher 场景：真实 HostSignals、原 Supervisor/NativeSession、父子独立非阻塞管道、后台文件读取/准备、纯 Engine 激活/确认；自身 create_new 文件从 OLD→96KiB padded NEW(Accepted2)→invalid(Rejected2)→重复invalid(Unchanged)→已接受正文(Unchanged、不产生3)→删除(ReadFailed)，只向自身父 PID 发 SIGINT，实际 child0/Cooperative、source回收。旧 held repeat/up 仍输出 b，新 down 输出 c。测试中临时文件写入是 fixture 驱动动作，生产 control loop 不同步写配置。

三个场景均无真实输入/输出设备、console provider、VT、驱动或安装；Active/session 证据与输入是合成的，终止报告仍 Missing、recovery_confirmed=false、successful=false。Windows 没有此 Linux-only source/watcher，前序 native 补验仍待执行。本轮没有新增 unsafe、FFI 或依赖；既有 ABI 未改变，不重跑 C oracle。

下一步在 run-linux 的冷 PreparedLaunch 中保存来源路径与初始精确 Arc，启动后将 ConfigWatcher 接入唯一 NativeSession 调度，补用户参数/手动操作/通知呈现与 actual-main 永久无输入准入测试。仍需图形会话、hotplug、真实console/硬件、Windows reload adapter/原生验证、advanced engine、UI/打包及原完整目标。验证详情见 [记录](validation/linux-2026-10-11-watch-scheduler.json)。
