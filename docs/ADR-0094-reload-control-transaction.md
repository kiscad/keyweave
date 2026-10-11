# ADR-0094：有界 reload 控制事务

日期：2026-10-11。状态：组合 controller 已实现；真实 host/worker 活动循环尚未接入。

## 事务组合

`keyweave-worker::reload_control::Controller<R,W>` 持有唯一 Receiver、ack writer、PreparationSlot 和固定 Target，构造时绑定 run/实际 active 代号与 1..30s 本地事务预算。不得在输入 callback 调用；tick 在 evidence 或 IO 之前执行 outside-callback 检查。它不运行输入 actor 的 poll/timers，不生成 progress、parent lease、session readiness，也不查询原生 provider。

状态按 Receiving→Received→Preparing→Ready→Acknowledging→Receiving 推进。每 tick 仅执行一个状态操作，最多一次 transport IO；Received 单独一 tick 才 take/spawn 冷任务，因此正文接收后的失效可以在准备线程创建前关闭。Preparing 只 poll 同一任务；Ready 沿用 ADR-0092 sealed actual actor delivery。Acknowledging 每步只写固定帧的剩余部分，完整写入并通过 post-check 后先将不可构造的 AckSent 交回 Receiver，再消费 AckWriter.into_completed_writer，复用同一个 owned writer 进行下一事务。未完成或关闭的 sender 被消费时会丢弃 writer，不交付端点。

Receiver 新增纯 receiving 查询：只有保留了首个 header 字节或事务才为 true。首个实际 header 字节的 PRE-IO 时间加 timeout 是固定本地 deadline，所有部分 header/body、WouldBlock、后台准备、激活与部分 ack 都不能延长；Idle WouldBlock 不建立事务预算，但独立父/会话租约仍然检查。第一步加法溢出失败；严格 at>=deadline 即过期。合法 ack token 后才清除本地 deadline，下一请求重新锚定。

## 独立证据与失效

调用方提供每次新鲜 Evidence：显式同域单调 at、parent_until、session_until 与 stop。tick 在状态操作前后各调用一次，要求 at 不倒退、stop=false、严格 at 小于两个独立租约和当前本地 deadline。Evidence 是受信外部观察，不是此 controller 生成的授权 token；真实控制循环还必须先验证 parent guard、Stop 通道、consumer/readiness/访问撤销，并从其原 clock/provider 提供正确期限，不能用任意远期数字取代实际证据。

操作错误立即永久 close，无需在失效端点上额外 IO。前后检查失败也 close，丢弃 owned reader/writer/ack copies 与未激活结果；原始继承 FD 仍遵循保留至实际进程退出的合约。停止不能抢占后台编译；原 slot/job 保留，reap_closed 在控制线程非阻塞观察同一任务直到回收，不重启、不激活、不写确认。返回 true 只证明没有 retained cold job，不证明进程退出或输入/输出恢复。Drop 未回收线程仍可能 detach，真实集成须保留 owner 驱动原恢复路径。

实际 activate 后 post-check 超时/撤销/clock failure 时，配置实际代号已经变化，controller 关闭且不写新的 ack，禁止 rollback。完整 ack write 后才发现过期时，已写字节也不能撤回；父端必须使用 ADR-0091 自身固定 deadline/post-read 校验，不能把迟到确认当成功。reader/writer 对象必须是可信 nonblocking owned 端点；generic Read/Write、线程/分配/已完成 join 没有硬实时保证。

## 接入要求

下一步由原生 control loop 持有此 controller，在实际 input poll/timer/Stop/parent-session 检查之间交替 tick，保持原 progress epoch 与独立监督不受准备任务影响。Controller.close 必须伴随原 runtime 的 capture-first stop/recovery，不能因为 reload 错误只丢管道却继续持有抓取。wait 需要包含独立 reload readiness 或有界 housekeeping；Preparing/Ready/ack 待推进不能无限等待输入。Windows CaptureSession 仍需其原 try-borrow/outside-callback 原生 adapter，此 controller 不暴露或绕过内部 actor。

本轮未修改实际 worker main、Linux host spawn/session loop 或 Windows CaptureSession loop，用户 watcher/reload 命令仍未提供。完整高级规则、桌面 provider、GUI、driver 和移植目标继续未完成。

## 验证

Linux Rust 1.97.1 locked/offline workspace/all-targets 916 tests、Clippy -D warnings、fmt、60 项固定来源检查通过。新增 8 项 portable scripted 控制测试与 1 项 Linux owned pipe 测试：两事务复用同一部分写 ack 端点/首成功次拒绝，Idle 与独立租约，部分 header 固定严格 deadline，post-header expiry 在 body/线程前关闭，pre-Stop/倒退零下一 IO，post-commit expiry 保留实际代号且零 ack，post-ack expiry/BrokenPipe 不 rollback，Preparing Stop 回收原任务，原生独立 pipe→后台 Linux 准备→实际纯 Engine→Accepted ack。

新测试使用显式模拟 Evidence；Linux pipe 测试没有 parent guard、实际会话 provider 或硬件捕获，不是完整 native control-loop 验收。既有实际-main no-input harness 随 workspace 通过。未新增 FFI/unsafe/dependency，C ABI oracle 无新边界未重跑。Windows 新增 portable 测试和前序 native 检查仍待补执行，此前自动审批额度问题未解决。见 [验证记录](validation/linux-2026-10-11-reload-control.json)。
