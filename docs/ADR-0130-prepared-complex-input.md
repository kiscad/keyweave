# ADR-0130：上游生成与Complex input登记的可组合准入

Simple阶段向Complex生成时，GeneratedStageQueue已有prepare，但Complex register_input直接commit，不能让两侧先全部预检。增加PreparedComplexInput独占arena/input map/last ID，prepare_input完成stage/current Arc/live device/ID/槽预算预检，commit才登记并推进ID，abort返回原owned ActionPlan而不变状态。register_input包prepare+commit保持原行为，zero计划仍只使用ID无根。

caller持有上游PreparedScopedGeneration和下游PreparedComplexInput，任一拒绝均drop另一guard；都通过后commit两侧，无业务拒绝（alloc failure/panic不包含）。此步是Full Simple/controller的根登记协议，不替代旧Simple Live父新child的全stage同arena证明，也尚未传播Simple consumed scope；native gate保留。

新增6tests：abort返原timestamp/Arc/target及drop保ID/source/input；目的device拒绝不提交已准备上游变量；8192源timeline拒绝不登记目的；16384目的source预算拒绝不提交上游变量；成功双预备生成真实Complex DueEvent→actor/Output变量Down/原Up/实际孩子完成后退休源ledger；zero root只有commit用ID。首轮6与原48actor测试通过，无初始失败。

Linux工作区1203用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-prepared-complex-input.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-prepared-complex-input/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝后Windows原生仍未补验，本轮无Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作。Linux/mock不是Windows证明。
