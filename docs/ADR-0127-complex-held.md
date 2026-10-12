# ADR-0127：Complex actor 的 held 定时替换

解除held冷gate，仅delayed仍拒绝。一个待触发held task保存slot与checked deadline=实际Down+旧threshold；任何新Down（含变量）取消旧task，再按新规则设置。目标原Up/所属设备控制/Reset撤销任务。deadline纳入next_deadline，caller先poll到期任务再交付同时间input；poll_with_context给timer最新上下文，旧poll默认Unknown。所有time显式注入，无线程/IO。

到期生成旧main的保存Up及逻辑scope/变量Up，按当时目的view/context一次filter held数组；held main保repeat/saved Up（非tapAll）。新child替换当前main关联，旧main仍是树child直到实际结束；FIFO保证旧Up/scope marker先于held实际Down。原Up释放新held child并继续alone/afterUp；held本身不取消alone，保持原规则timeout。halt仍在生成时压制后续to。

timer工作一次产一个空输出step，实际intents由后续poll交付；旧child清理不可丢输出。生成拒绝可在旧Up提交后latch Failed，须stop/reset，不回滚旧Up或证明OS恢复。输出due先drain，timer只在无当时due输出时生成；完整allstage调度/物理EOF/source池/Context/native duration/Windows/hardware等继续，native gate不改。

来源为固定 `basic/to_if_held_down.hpp:60` 的debounce/旧Up→modifier恢复→held生成顺序、`:123` 任何key_down取消以及 `basic/basic.hpp:182` 在validity判断前调用cancel。旧Rust replace_held先释放旧children/恢复scope，再fresh filter，且不取消alone。

新增9tests：阈值deadline及一次替换保存Up；提前Up取消/等于deadline先poll；变量新Down取消；main variable Up之后过滤held且alone资格仍存；generation context在actual FIFO等待后仍用冻结数组、原Up早于未来旧array；旧rule跨reload及设备收尾；deadline overflow零父/输出提交；empty/halt held在u64::MAX无新ID但释放旧main；held modifier scope替换和原Up恢复。

首轮修改script在写入actor后未匹配多行旧gate fixture，现有27测试中26通过/1旧held-gate断言失败；只更新fixture到delayed（剩余未接入分支），随后原27/新增7与最终9全部通过。Linux工作区1185用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-held.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-complex-held/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝后Windows原生仍未补验，本轮无Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作。Linux/mock不是Windows证明。
