# ADR-0124：拥有 producer 与 Output 的 Complex main 执行器

前序原语接入 ComplexStageActor：私有拥有 InputBatch 原计划、BindingArena<ProducerDecision>、每slot主child关联、OutputStageActor及单调batch IDs。注册已生成的当前Arc Complex输入计划，再交付Down/Up；Down只选择一次并共同提交实际producer和main child（empty走消费路径），Up共同标记并准入保存Output Up。poll交付一个Output步，实际child完成后先拆外部登记，再在原Up已标记时finish producer；根所有slots释放才retire。输出intents不会被内部drain丢弃，due输出要求caller先poll，再重试尚未选择的输入。

admit错误在可能已pop输入后使actorFailed，禁止普通输入直到reset，不重新选择已失败Down；reset/设备控制先清Output实际绑定再清producer树，返回intents及必须丢弃的上游输入batch IDs。任何结果均不证明OS接受/恢复，native所有者仍须先停capture、按submitted ledger恢复。

这是完整controller的Complex main组件，当前冷构造/activate明确拒绝Complex held/alone/after-up/delayed分支，不忽略这些配置；simple规则由上游负责。支持main数组duration、modifier scope、lazy、variable、halt元数据与empty consume。Simple actor、跨stage scope差分、所有阶段共享来源/ID预算与高级分支/context scheduler尚未接入，native duration gate/默认runtime保持，不把此单stage组件说成完整移植完成。

输入新root必须current Arc并用next_batch_id注册；旧已登记root可跨reload交付原事件。旧Simple父在reload后新派生Complex root需要后续全stage同arena协议，当前严格root入口不能替代它。全树预算仅在本actor的Complex roots/Output child共用arena；实际source ID allocator/native Acceptance尚未统一。

附加modifier的原Up允许与Down不同lazy：固定上游 `basic/event_sender.hpp:188` 对modifier主目标的附加modifier Down可非lazy，`:250` 保存原Up lazy=true。ProducerDecision.with_original_up保已注册原recipe的Up flags，passthrough计划保该flag；默认API仍保原Down lazy。Terminal执行Up时给实际Leaf使用planned Up.lazy，而非只记Down flag。原有last-same-device-batch完成sync策略保持；完整controller仍须协调真正物理绑定EOF和Simple scope差分，不能把本组件用作完整阶段调度证明。

新增12个回放：保存输出Down/Up跨reload自动退休；early originalUp等待50ns数组future c；empty main保父直到Up且无fallback；两个设备同键owner和单设备清理；due输出先poll再同receipt重试；坏Down使Failed而Reset只清实际Down；exact halt batch和上游halt元数据；高级分支new/activate明确拒绝不换配置；注册附加modifier非lazy Down/lazy Up透传并在下一普通键同步；变量生成影响下一选择但旧键Up不重选；伪造过早输入tap Up拒绝；Complex consumed modifier的生成可见性和实际lazy恢复。

初轮8通过/1失败：fixture在同设备所有output batches结束后仍期待latent Shift Up，但既有Terminal EOF会sync；加入同设备另一持键验证EOF前lazy行为，9通过，再新增3个未覆盖的变量/输入duration/Complex scope组合测试，最终12通过。已有Output/producer Up回放通过。Linux工作区1161用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-main.json)。

命令：`RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`；Rust1.97.1，logs在 `target/validation/linux-20261012-complex-main/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。Windows原生仍因此前自动审批额度问题未补验，无绕过或Windows调用；无真实输入、设备、driver、安装或硬件操作。Linux/mock不是Windows证明。
