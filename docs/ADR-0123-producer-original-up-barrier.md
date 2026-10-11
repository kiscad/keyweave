# ADR-0123：producer 原 Up 准入与孩子完成屏障

producer需要校验原Up的slot/stage/device/source/旧Arc/target/lazy，并允许同刻迟交付的Down/Up。ProducerDecision.prepare_up只预检并持有独占决策，commit才记录up_delivered_at；drop/abort不标记。caller先预备父Up，再准入孩子保存Up，成功才commit父标记，任何孩子拒绝仍能用原receipt重试。

BindingArena<ProducerDecision>.prepare_producer_up只对实际Live payload调用上述事务，无任意live_mut出口。finish_producer要求原Up已经commit，随后复用原take的孩子完成阻挡；子登记仍存在则ChildPending，不能以Up生成或交付代替实际孩子/scopes清理。空main在合法原Up后直接释放原payload，没有假Up输出；完整controller还须安排AfterUp/Alone分支再finish。

receipt为caller回放一致性非native认证；标记不证明下游或OS已恢复，也不记录全actor时钟。泛型take/cancel供明确控制清理仍保原语义，普通原Up路径应走finish_producer。完整controller/scope/source池/分支/native duration/simultaneous/Windows补验/硬件等继续，duration gate保留。

新增8tests覆盖abort/drop/同receipt重试/一次提交、slot/stage/device/source/foreign Arc/target/lazy/halt/因果时间错配、Reserved/unknown/Released拒绝、Up提交后Reserved child仍阻挡finish、保存Output Up跨reload直到实际完成且登记拆边后才finish、8192下游满队列拒绝不提交父标记并同receipt重试、empty父在原Up后无假输出释放，以及变量key_up_value改变拒绝。允许earliest Up110与迟交付Down125同时actual125的正确FIFO情形。

首批6tests及最终8tests均通过，无初始失败；Linux工作区1149用例、Clippy -D warnings、fmt和source60通过：[验证记录](validation/linux-2026-10-11-producer-up.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`。logs在 `target/validation/linux-20261011-producer-up/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。Windows原生仍因此前自动审批额度问题未执行，无绕过或Windows调用；无真实输入、设备、driver、安装或硬件操作，Linux/mock不能证明Windows完成。
