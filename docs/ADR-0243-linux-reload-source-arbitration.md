# ADR-0243：Linux 文件监控与外部 Apply 的确认归属

问题：ConfigWatcher 是已有私有 reload outcome 的独占消费者，公开 Apply 不能再直接 take 同一确认。外部 Apply 成功后，未变化的监控文件也不能自动把配置改回旧版本。

选择：在既有 scheduler 内新增单个外部 ticket 槽与 Apply Backend adapter，仍由同一 scheduler 驱动 NativeSession 并独占取 ACK。NativeSession 首次绑定 source owner 后永久保留归属：原始 submit 被拒绝、原始 take 不消费确认，第二个 coordinator 不能接管；已经绑定的 coordinator 也不能凭相同 run ID 转绑另一个未绑定会话。外部请求只在没有文件读取/候选/在途请求、没有未领取外部结果且 peer Active/空闲时入队；不抢占文件候选。ticket 为进程内 coordinator owner+单调序号，不携带私有 worker run ID。读取结果只匹配原 ticket；错误 ticket 不消费结果。

同一次实际 ACK 校验 base/target 后更新 active，并将外部结果保存到单槽供 Apply Transaction 领取。seen 仍代表最后观察的文件正文；外部 Apply 不改变 seen。因此相同文件返回 Seen，不回滚外部配置；真正改变的文件仍可形成下一次热更新。文件确认不会覆盖尚未领取的外部结果。

增加不监控文件的 coordinator 构造入口，供未来仅 GUI Apply 的 host 使用；无目录通知/读取任务。放弃外部 ticket 仅放弃回复归属，不取消在途更新；实际 ACK 仍由 scheduler 消费并更新 baseline。关闭或丢 ACK 保持不确定，不能制造 Cancelled。提交调用先锁住重试；错误或 panic 后不得重新提交，仍监督原 owner。

本轮接真实 NativeSession 的 adapter 与既有 host watcher 的生产 scheduler，并增加确定性仲裁测试和无输入实际 worker/私有 ACK 组合验收。公开 socket/pipe、CLI 与 GUI Apply 按钮仍须后续接线；Windows 尚无对应 source owner。不能据 Linux 组件通过宣称完整跨平台 Apply 已完成。


放弃/关闭边界：`ApplyBackend::queued_ticket` 在最初 submit 返回 Queued 后可取出本地 ticket，即使公开 Transaction 后置时钟检查失败也要保留，用于 `abandon_apply`。Pending 放弃仍等待并消费真实 ACK，Accepted 后更新 active；ready 结果可立即丢弃。关闭 coordinator 后不再收集公开 completion，pending 保持 Unknown；原 NativeSession 仍需继续观察原进程退出，不能以 close 冒充取消。缓存中已完成的结果仍可作为历史事实领取。

验证：新增7项确定性仲裁测试，既有实际 linux_reload harness 增加 apply/apply_lost_ack 两种无输入模式，覆盖实际私有 ACK/编译拒绝/激活后丢ACK、原始接口不能偷确认、第二个 source owner 拒绝及同 child Stop/实际退出。全量 Linux 2038项通过/8项 native Unix socket 默认忽略；格式、Clippy、源码62、regex oracle、Linux ABI、X11 SDK 共七项检查通过。最终增加“相同 run ID 不能转绑新会话”检查后重跑全套。没有打开输入设备或执行注入；public socket/pipe 与 GUI Apply、Windows adapter 仍待实现。见 [验证记录](validation/linux-2026-10-16-apply-arbitration.json)。
