# ADR-0125：Complex actor 的 after_key_up 分支

解除仅after_key_up的冷gate，held/alone/delayed继续明确拒绝。原Up先准入保存main Up/逻辑scope恢复及变量key_up_value，commit父Up标记，然后使用保存rule和当前目的生成视图/Up-time context一次过滤AfterKeyUp数组，tapAll计划FIFO接在main全部Up/scope marker之后。非空分支登记为同父的另一个child；empty/haltskip不创建child。父仅在main和分支均实际完成并清登记后finish。

main/分支中halt在成功生成提交时记录，实际output receipt继续保exact关联；incoming halt仍只属于上游规则。复用生成时过滤的数组，不到actual delivery时重选。若main Up已提交而branch准入失败，actorFailed并停止普通poll/input，须capture-stop后reset/按submitted ledger恢复；不回滚已生成Up或谎称全事件原子。

poll完成任一child只在它是main时清main关联，不能让branch覆盖或提前清main。设备/reset取消未来branch且仅释放实际output owners，不为未执行Down编Up。Simple/controller、跨stage scope/物理EOF/global source IDs、held/alone/delayed/context scheduler/native duration/Windows补验/硬件及原目标继续。

新增8tests：empty main afterUp child duration完成才释放父；Up-time context在branch实际执行前过期仍复用已过滤数组，FIFO保main50/branch20；main variable key_up_value影响branch且整数组先过滤再由自身variable更新view；future halt立即压制afterUp/incoming halt不污染新rule；旧rule跨reload且设备清理只收实际分支owner；8192队列拒绝发生于main Up已提交之后，Failed后reset只释放实际b/c；Reset在future branch Down前不造c/d Up；filtered empty在u64::MAX batch counter仍无需分配而完成父。首批6与最终8新增测试均通过，无初始失败。

Linux工作区1169用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-after-up.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-complex-after-up/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝之后Windows原生检查仍未补验，本轮没有Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作，Linux/mock不是Windows证明。
