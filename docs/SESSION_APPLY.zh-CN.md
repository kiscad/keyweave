# 公开配置 Apply 协议与开发接线要求

这是后续 Linux/Windows 设置界面 Apply 所需的公共事务层，当前实现位于 `keyweave-progress::session_apply`。它尚无 OS 传输或用户可执行 Apply 命令，不表示界面热更新已经可用。Status/Stop 的 KWCC/KWSC 和 worker 私有 KWRQ/KWAK 不变。

## 帧与身份

所有整数使用 little endian；请求和回复都固定48字节。请求头之后的配置正文为1..8MiB精确原始字节，保留 JSONC、UTF-8 和换行；codec 不解析或执行配置。传输层必须先验证头，再有界分配正文，并证明完整请求结束后才能创建 Transaction。

| 偏移 | 请求 KWAQ | 回复 KWAR |
| --- | --- | --- |
| 0..4 | ASCII KWAQ | ASCII KWAR |
| 4 | version=1 | version=1 |
| 5 | 保留，必须0 | outcome=1..7 |
| 6..8 | 保留，必须0 | 保留，必须0 |
| 8..16 | 非零公共请求 ID | 对应公共请求 ID |
| 16..24 | 预期公共 session nonce | 被观察 host 的公共 session |
| 24..32 | 非零基础配置代号 base | 请求 base |
| 32..40 | target，严格 base+1 | 请求 target |
| 40..48 | 正文字节长度 | 已确认的配置代号，0表示未知 |

帧不携带 worker run ID、私有请求序列、按键事件或配置路径。公共 nonce 是实例 fence，不是认证 secret；传输层仍须认证同用户及所选 host，不能把可解析的帧当授权。请求 ID 仅在调用者事务上下文中关联回复，协议没有跨连接的全局去重缓存；客户端不得对不确定的请求自动重试。

| outcome | 值 | 必须证明的事实 |
| --- | --- | --- |
| Committed | 1 | 对应 ticket 收到实际 Accepted，确认代号为 target |
| Rejected | 2 | 明确提交前拒绝，或对应 ticket 的实际 Rejected；确认代号为 base |
| Cancelled | 3 | 对应 ticket 的实际 Cancelled；确认代号为 base |
| Busy | 4 | 本次提交没有排队；独占 reload lane 正忙 |
| Unavailable | 5 | 本次提交没有排队；当前 host 不可提交 |
| StaleSession | 6 | 预期 session 与当前不同，本次没有提交 |
| StaleGeneration | 7 | 当前代号不是 base（或未知），本次没有提交 |

`Reply::matches` 还要求 id/base/target 对应原请求。StaleSession 必须返回不同 session；其他回复必须属于预期 session。矛盾的 Committed/Rejected/Cancelled/StaleGeneration 代号会被 codec 拒绝。观察代号是该回复所依据的确认事实，不保证之后没有其他配置更新。

## 协调器

`Transaction<Ticket>::new` 消费完整不可变正文，验证长度并建立绝对 deadline。开始时间必须从首次观察请求算起，不能在正文接收完成后重新起算。timeout 非零且最多30秒，溢出、时钟倒退、到达期限都终止事务；传输和宿主也要遵守该期限。

`step` 接收宿主已有的 Snapshot，并最多调用一个 Backend 方法：

1. Ready：先检查 session、base、Active/fault，再一次性移动正文给 `Backend::submit`。Queued 只进入 Awaiting，不能产生 Committed 回复。
2. Awaiting：只查询同一个 ticket 的 completion。Pending 不延长期限；Accepted 必须为 target，Rejected/Cancelled 必须为 base。
3. Finished：保留冻结的回复，不再调用 backend。它是既有事实缓存，不是新的状态观察或新的发送期限。
4. Failed：保留错误与 `may_have_applied`。backend 调用开始前就进入 Calling 并锁住重试；panic 后再次 step 也不能重放正文。

backend 调用前后均检查时间。调用错误、错误 ticket/代号、提交后的 session 变化、超时或 ACK 丢失不能被解释为未应用。提交前的明确拒绝，以及经过校验的实际 Rejected/Cancelled 回复，才证明本事务没有应用。Committed 表示这份正文曾被确认激活，不代表输入已恢复、设备已就绪、进程退出，或配置永远保持当前。

Transaction 不拥有 worker/Job/设备，也不会因 Drop 停止 worker 或回滚配置。即使客户端断开，宿主仍要监督并回收原来的同一进程。若激活后丢失回复，客户端必须显示不确定，并通过后续观察/用户明确操作处理；不能把重试当幂等。

## 生产接入必须完成的工作

1. 提供有界 Linux/Windows 传输：头校验后才分配最多8MiB；限制连接/候选数量、每次 IO 和总期限；在调用 backend 前完成消息终止和 peer 认证。保留独立 Status/Stop 的响应能力与监督优先级。
2. 为实际宿主建立独占 Backend adapter：ticket 绑定真实私有 reload 请求、精确正文/base/target；只从该请求的实际 worker ACK 生成 Completion。不得依据公共 Status 的代号变化猜测“这份正文已应用”。
3. 统一 watcher 和 GUI 的 reload source/outcome 仲裁。现有 watcher 独占确认队列，不能让两个消费者并行 take 同一个 ACK；外部 Apply 成功还要更新 active baseline，并定义文件未变时不意外回退的策略。
4. GUI 请求绑定 endpoint/session/base、文档精确正文/revision、目标与执行模式。文档或端点改变时旧结果只能显示历史事实，不能使当前草稿显示已应用。按钮要显示 Pending/Committed/Rejected/Unknown，不把保存副本当运行时 Apply。
5. 增加真实 host→实际 worker→私有 ACK→公开回复的无输入验收，覆盖错误 session/base、busy、编译拒绝、实际激活后丢 ACK、Stop 并发和 owned child 退出；分别跑 Linux 和 Windows 原生。最后再进行授权的实际输入/持键恢复与桌面测试。

现有10项新增测试包括8项确定性协议/状态机测试和2项真实冷编译→Engine.activate 的组合测试。组合使用 test-only ticket adapter 和合成 Engine 输入，不是实际 worker ACK 或原生输入验收。完整接入前不得启用 GUI Apply 或声称平台运行时链已验证。参见 [ADR-0242](ADR-0242-public-configuration-apply-transactions.md)。
