# ADR-0118：父子登记树的 snapshot 额外来源预算

## 问题与选择

ADR-0117 的上游登记只计算 Down 槽，而 snapshot scope 还预留额外 modifier 来源。在一棵已有16382槽的producer树中，新child的一个Down和两个snapshot来源会超过16384，旧上游检查只看一个Down而允许登记。增加 BindingArena 的 extra_sources 计数及每批持久预留，reserved_sources=reserved_slots+extra_source_reservations 统一限制为16384。

Reservation.with_extra_sources 只增加预备批次预算，不改arena，失败消费/丢弃整个guard；commit才同时加入槽位与额外预算。所有后续prepare也计既有extra，防止其他批次只计slots越界。take释放实际payload不会提前释放批次预留，retire/cancel/cancel_subtree移除该批次时才回收extra；Owned取消记录提供extra_sources数量，仍不表示真实owner已恢复。

OutputStageActor.prepare_child_scoped 自动把非空scope.reservation加入上游Reservation，然后准备两输出侧。额外来源、父子边、变量/逻辑scope、实际预留三方共同commit或共同abort，调用方不要重复手动计同一输出scope。既有 reserved_slots 保原身份槽数量，避免把extra当ActionSlot或fake Live绑定。

## 边界

此步统一一棵BindingArena树中的slots和extra预算，包括已接入的输出snapshot child；完整producer必须为Simple/Complex本身的额外来源使用同一机制，所有根和子树必须在同一拥有者的arena内。裸Output roots、多个独立arena与实际source ID allocator尚未统一，不能将本局部计数说成整个运行时source池已经完成。

输出自己的预算和实际scope生命周期仍保持；即使输出完成，上游extra仍等待controller清理登记，不能提早复用。caller必须先结束实际子绑定/作用域再清理登记，任意外部remove并不是native恢复或全树完成证明。普通分配失败/panic不在commit业务原子保证内。

native duration gate、完整Simple/Complex actor/scope传播/context/分支/simultaneous/Windows补验/硬件及原目标继续。

## 验证

新增4个arena预算测试与5个Output child组合测试：精确16384边界、usize::MAX/累加预备abort、后续slot准入、subtree原预算回收、snapshot overflow、三方abort/terminal拒绝、实际输出完成/设备清理后等待登记确认，以及main scope不收snapshot额外成本。focused 23 tests（含原14）通过；工作区1099用例、Clippy -D warnings、fmt及source60通过，初始无失败。

命令：`RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`。详见[验证记录](validation/linux-2026-10-11-binding-source-budget.json)，logs位于 `target/validation/linux-20261011-binding-source-budget/`。

此改动无unsafe/FFI/dependency/native ABI变化，未重跑ABI oracle。Windows原生检查仍因此前自动审批额度问题未执行，没有绕过或调用Windows；Linux/mock不作为Windows证明。没有真实输入、设备、驱动、安装或硬件测试，生产duration gate保留。
