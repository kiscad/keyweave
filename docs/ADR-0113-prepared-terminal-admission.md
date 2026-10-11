# ADR-0113：终端执行器预备事务

## 问题与选择

ADR-0112 的生成视图已可预备，TerminalActor 的 enqueue 仍立即提交，阻止调用方对生成和执行两部分进行全部校验。增加 PreparedTerminalAppend：prepare/prepare_scoped 保存时间线 guard、BindingArena reservation、待插入 Batch 与来源/ID/scope预算的引用和预计算值；丢弃 guard 无状态变化，commit 消费它且没有新的业务检查。原 enqueue/enqueue_scoped 包装预备再提交，保持原 Output/currentArc/严格递增BatchId/来源和容量语义。

调用方可先 GeneratedStageQueue.prepare_scoped(&plan)，再 TerminalActor.prepare_scoped(plan)，两者都成功才提交；第二方拒绝时丢弃第一 guard 即可。原 scoped wrapper 借用不延续到生成 guard，因此仍能被终端消费；不复制整份规则或配置。反方向准备也可以使用预先保留的生成 events。

预备不打开实际 modifier scope，不创建实际 Down，也不推进 source cursor 或 last_batch。commit 后仍要实际 poll 执行，并通过保存绑定完成原 Up 和恢复。空计划仍保原成功提交推进 BatchId 的规则，但没有Batch/slot/scope。

## 边界

没有把任意两独立队列包装成完整stage actor：双方时钟、FIFO尾及预算各自校验；生产调用方必须协调实际交付/退休和scope差分。saved original-Up 两边联合准入、完整Simple/Complex actor、旧父新孩子和统一来源预算仍待。不能解除 UnsupportedActionTiming，也不能把队列成功当OS接受。分配失败/panic 不在无业务失败的commit保证内。消费计划后的错误/abort 不自动退回原计划，调用方需要保留自己的recipe以重试，延续原 enqueue 的所有权合约。

## 验证

新增8个集成测试和1个私有cursor边界测试：raw abort不推进ID/预留/变量/Down；terminal device拒绝不隐藏生成侧consumed modifier，补device后原ID成功；生成侧交付身份冲突丢弃terminal预备，实际变量只在poll更新；两侧main准入成功后实际Down隐藏shift、原Up关闭scope并由实际结束协调生成账本退休；snapshot variable生成Up值2与实际Down值1/50ns后Up值2区别、额外source退休；empty abort不推进ID但成功empty仍推进；pending取消不打开scope/不造Up；执行队列精确8192事件满时拒绝不赋生成变量/不留预留，取消自有pending批次后原ID可重试；source cursor设u64::MAX-2，abort原cursor和ID不变，commit两个slot才耗尽，后续拒绝仍不推进。

初次新增私有测试编译使用不存在的 KeyId::keyboard，已改既有 KeyId::new(7, usage)，未改业务实现。focused tests及既有TerminalActor/modifier scopes测试通过，随后 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline` 共1042个用例通过，`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check`、`python3 tools/verify_sources.py`（60文件及fixture）通过。无unsafe/FFI/依赖/ABI变化，未重复ABI oracle。无原生输入或设备操作；Windows补验仍待此前自动审批额度问题解除。

[验证记录](validation/linux-2026-10-11-prepared-terminal-admission.json)。
