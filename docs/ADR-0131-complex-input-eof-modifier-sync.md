# ADR-0131：用已登记input生命周期控制末端modifier EOF sync

Terminal原策略在同device最后output batch完成时sync。Complex input group可仍有Reserved未来slot，此时lazy modifier Up过早被sync。InputBatch新增unreleased计数（原Up成功提交才减，区别于实际child完成的remaining），Complex poll提供private per-device EOF predicate：仍有unreleased输入则不做terminal末批sync。低级Terminal/Output public poll保原策略。

如果最后输入为empty/tap已完成，原Up不会产生新output batch EOF，因此admit原Up后在该device无未释放input且无output recipe时显式sync，返回intents不能丢。已原Up但等待delayed的父unreleased=0，不延后物理device EOF到timer；其它device按原全局modifier同步模型保语义。

这是Complex input group结束协议，仍非完整物理Simple root EOF/跨stage scope证明。完整controller需把真正物理绑定生命周期接到该policy；native gate不改。新增5tests：附加shift非lazy Down/lazy Up之后仍有Reserved x slot，不flush到x实际Down；最后x被empty rule消费则其原Up显式flushshift且无output batch；已原Up但保delayed的x不将EOF延后到task；Reset在latent shift+future x时只收真实emitted shift；bare Output public poll依然末device batch结束flush。首轮5与原48actor回放通过，无初始失败。

Linux工作区1208用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-input-eof.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-complex-input-eof/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝后Windows原生仍未补验，本轮无Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作。Linux/mock不是Windows证明。
