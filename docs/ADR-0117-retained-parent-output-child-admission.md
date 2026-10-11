# ADR-0117：旧父绑定到输出子批次的联合准入

## 问题与选择

OutputStageActor 的普通新批次必须来自当前 Arc，但已经实际存活的上游 Complex 父绑定可以在 reload 后生成使用原配置的子批次。不能全局放开旧 Arc，也不能先提交输出变量再登记父子边。

BindingArena 的 child Reservation 已独占 arena 并预检父 Live、下一阶段、device/source/旧Arc、容量。以该预备事务产生 crate-private ChildAdmission，精确匹配 child batch/stage/start/device/source/Arc/Down槽数。OutputStageActor.prepare_child/prepare_child_scoped 消费完整 Reservation，预检生成/执行两侧，并返回同时持有三份预备状态的 PreparedOutputChild；commit 才同时登记父子边和输出批次，abort丢全部状态。普通root准入继续严格currentArc。子批次只接受exact current Arc或更旧generation；future及same-generation foreign Arc不是旧父快照。

新增 OutputStageActor 的普通 prepare/prepare_scoped 供组合，enqueue继续包装prepare+commit。Output recipe保存parent slot作为只读关联；saved original-Up不需重新授权旧Arc，实际交付/重复资格/退休仍用原流程。旧Arc schema与变量引用保持原验证，只有既已保留的变量名字可使用，不在Reset后重建消失的旧namespace。

## 边界

上游 BindingArena child节点在commit后是Reserved登记，实际owner在OutputStageActor的私有实际绑定表。Output完成或取消后，上游控制器还须根据同一BatchId清理登记再释放父；输出actor不拥有外部arena，不能将自己退休当上游整个子树完成。该登记不为未执行slot编造Live payload或Up。完整Simple/Complex actor与跨stage source预算/作用域传播仍待。

此凭据证明库内保存绑定/预备事务的一致性，不是native会话或跨用户鉴权。Rust调用方负责上游Live payload确实表示其实际producer binding。源树与输出仍各自有界，snapshot额外来源尚未纳入统一全输入source池。empty计划没有Down，不能用正槽child Reservation伪造空登记；普通empty root路径保持。

commit无业务拒绝，普通分配失败/panic不在原子保证内。native duration gate、Windows补验/硬件/UI/driver等完整原目标继续。

## 验证

新增14个集成测试，Producer payload不要求Clone/Debug：oldComplex Live父在reload generation2后授权旧b child，输出原Up完成后父仍阻挡直到清理Reserved child登记，返回记录保旧Arc且无假实际payload；普通旧root与parentless reservation拒绝而原ID可用于合法root；三方prepare abort不改变量/队列/边，同child重试成功；6类origin失配（batch/start/device/source/Arc/stage）及Down槽数失配零状态变更；精确8192满队列拒绝丢上游预留，释放后同child成功；512 distinct keys拒绝时不提交已准备modifier或父边；frozen old main consumed scope原Up恢复shift；old snapshot variable保生成值2/实际Down值1/Up值2及额外source预留到完成，外部arena2slots/输出3slots差异明确；Output Reset不重建消失的旧变量namespace；Reserved父不能授权/正槽登记不能接empty child；旧schema非法即使metadata一致仍拒绝；future及same-generation foreign Arc拒绝；普通root预备abort不推进ID，较小ID可提交；exact current Arc child正常登记并完成。

focused tests及既有OutputStageActor/BindingArena测试通过，无首次新增测试失败。随后 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline` 共1090个用例通过，`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check`、`python3 tools/verify_sources.py`（60文件及fixture）通过。无unsafe/FFI/依赖/native ABI变化，未重复ABI oracle。仅纯状态及已有无输入检查，无Windows原生、真实输入、设备、安装或driver操作。

[验证记录](validation/linux-2026-10-11-output-child-admission.json)。
