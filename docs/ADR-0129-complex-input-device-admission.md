# ADR-0129：Complex input 的显式设备代际准入

Complex register_input此前只验stage/current Arc/ID，真正device检查延至Output timeline events。empty root和matched empty main无事件，因而never-observed/removed/旧代际/future-unobserved设备能登记甚至激活空父。增加timeline只读check_live_device并由Generated/Terminal/Output窄wrapper共享两侧历史，Complex register_input在任何source/ID/input变更前检查精确已观察且未removed代际。

普通非空和空输入一律；reject不latch actorFailed、不占counter/预算、同ID可重试已知合法设备。device控制可以登记下一代，Reset保历史/tombstone；空计划不应绕过平台已知设备模型。该检查仅库内history，不验证真实设备FD/native lease，不能证明硬件在当前时刻仍存在。

低级ActionPlan/Generated/Output空计划接口保原纯计划语义；由拥有input控制面的Complex actor执行新增边界，不全局收紧纯selection。Simple/全stage/native/Windows/硬件等完整目标继续。

新增3个组合回放：空与非空计划在unknown/removed/stale/future-not-observed代际均reject且无counter/预算/fault变化；同ID在显式g2观察后可注册并执行matched empty main/原Up；Reset保removed tombstone，g1不能重新观察/注册而g2允许。首轮新增与既有45actor tests全部通过，无初始失败。

Linux工作区1197用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-device-admission.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-complex-device-admission/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝后Windows原生仍未补验，本轮无Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作。Linux/mock不是Windows证明。
