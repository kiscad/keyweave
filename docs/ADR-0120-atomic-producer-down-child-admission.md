# ADR-0120：producer Down 激活与孩子准入共同提交

## 问题与选择

ADR-0117/0119 接受实际 Live 父的孩子，但真正的阶段执行器在交付 producer Down 时，必须先 arena.insert 才能 prepare 孩子。若下游队列/来源/变量/输出容量拒绝，父已 Live，不能把这一步宣称整次准入原子。

增加 BindingArena.prepare_child_with_down，接收已交付的 DueEvent<PlannedAction>、待保存的 producer payload 与子批次 origin/槽数。它只允许 Reserved 父，验证实际 Down 的slot/edge、stage/device/source/旧Arc、时间线因果关系，以及严格下一阶段孩子的同一来源和快照；不改变arena。Reservation 私有保存 pending parent activation，与原孩子登记/extra预算共同持有独占arena。commit先激活实际父，再登记孩子边；drop/任何后续拒绝不激活父。

原 prepare 仍只接受 Live 父。新 guard 可直接被 GeneratedStageQueue.prepare_complex_child_scoped 或 OutputStageActor.prepare_child_scoped 消费，形成父Down激活、子登记、生成/实际输出的完整联合准入。公开 API 只校验调用方持有的回放receipt一致性；DueEvent 本身不是OS认证或不可伪造的queue权限。

## 边界

父payload在prepare时必须是尚未执行副作用的保存状态，不能把真实native操作放进构造/Drop中。生成队列pop已经交付并非可回滚；完整controller须保存原frozen decision用于拒绝重试或进入失败恢复，不能重选条件制造另一动作。本事务只防止新实际producer绑定/孩子/输出的半提交，不撤销更早的生成状态或证明平台接受。

父原槽已占预算，激活不多收来源单位；子Down槽与snapshot extra仍按一份arena总预算限制。parent必须为Reserved，不允许重放Live/Released或将Up/提前未到期事件冒充Down，Output没有下一阶段。子origin.start不得早于actual delivered_at，防止迟交付后把孩子放回过去。allocation failure/panic不在commit业务原子保证内。

完整producer/controller、跨stage scope传播/source ID池/context/分支/native duration/simultaneous/Windows补验/硬件/UI/包装/driver等继续，生产duration gate保留。

## 验证

新增11个回放：commit激活原payload/不重复收费/孩子阻挡原父Up、预备extra后drop零变更、Down receipt的Up/Repeat/slot/stage/device/source/foreign Arc/时间错配、Live/Released重放拒绝、child origin/0槽/容量/Output父无下一stage拒绝、旧parent与snapshot Output共同abort后同计划重试、512 terminal keys拒绝已预备变量但不激活父、8192 Complex timeline拒绝后同frozen plan重试、snapshot来源超16384时父保持Reserved、实际Complex DueEvent迟交付125→Output child共同commit后原Up叶先父清理，以及rebase earliest120/delivered125/child start130保原parent origin100。

首次focused编译错误仅因测试用了不存在的KeyId::consumer，改为已有KeyId::new(CONSUMER_PAGE, usage)并移除未用helper；随后11测试通过，无生产行为修补。Linux工作区1120用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-11-producer-down-child.json)。

命令：`RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`。logs保留于 `target/validation/linux-20261011-producer-down-child/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle；验证后仅澄清API注释并复查fmt，无可执行逻辑变化。Windows原生检查仍因此前自动审批额度问题未执行，没有绕过或Windows调用；无真实输入、设备、driver、安装或硬件操作，Linux/mock不是Windows证明。
