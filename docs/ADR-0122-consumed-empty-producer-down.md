# ADR-0122：匹配空 main 的 producer Down 联合激活

## 问题与选择

ProducerDecision 保持 matched-but-filtered-empty 为consume。此时仍须保存实际父绑定直到原Up/分支调度，但不应为它登记任何孩子Down、来源槽、queue事件或实际owner。ADR-0120 的 child Reservation 要求正Down槽，不能用它伪造0槽孩子，也不能先insert父再尝试空输出准入。

BindingArena.prepare_consumed_down 校验Reserved父Down、下一stage origin、同一device/source/旧Arc和实际交付时间，预备单独的 PreparedConsumedDown。它不插入任何子batch，commit只激活原父。crate-private admission使用精确origin/0槽匹配，仅供空计划。GeneratedStageQueue.prepare_empty_complex_child_scoped 和 OutputStageActor.prepare_empty_child_scoped 消费完整guard，要求plan events和releases都为空，联合目的生成/执行准备；任何拒绝/drop/abort不激活父。

普通root/非空child路径保持原限制。Output空计划按原规则只消费strict batch ID，无recipe、parent child edge、source预算或输出。Complex空生成返回up_generated的原Up凭据，无交付batch/queue作用。controller仍须保存父ProducerDecision及分支/原Up状态；空main不应导致提前释放物理绑定或禁止原规则后续分支。

## 边界

Down receipt仍是caller提供的回放一致性，不是native权限。payload不得预先执行副作用，原生成/pop不回滚，控制器必须保留frozen decision用于重试/失败恢复。空计划本身不证明是某一条件选择的结果，caller须保持与原Decision一致；公开库与原Live payload模型一样承担caller语义责任。

不新增子登记，因此已有满arena可激活其原Reserved父；目的actor自身原有容量/strict ID/schema/device检查仍可能拒绝，失败不半提交。无snapshot额外来源，因为空scope不打开；未来分支有自己的正常child登记和预算。

完整controller、child Up/完成屏障、scope传播/global source IDs/context/分支/native duration/simultaneous/Windows补验/硬件/UI/包装/driver仍待，生产duration gate保留。

## 验证

新增9个回放：旧配置empty Output main只激活原父并使用strict ID而不产生child/owner；abort/drop保原父/ID且合法原plan可再次预备；旧Simple父与Complex empty共同commit返回no-op Up凭据；Consumed Down的receipt/next origin错配与Live/Released重放拒绝；nonempty计划拒绝且Output不latch Failed；empty origin batch不匹配及错目的queue不激活；满16384 arena在原槽内激活无新预算；满4096 Output batches让空目的准备拒绝不半提交，清一批后同main重试；空main旧rule在reload后手动生成AfterKeyUp branch，150 Down/170 Up完成后才能清child并释放父。

初始focused9 tests全部通过；既有producer Down/decision回放也通过。Linux工作区1141用例、Clippy -D warnings、fmt和source60通过：[验证记录](validation/linux-2026-10-11-empty-producer.json)。

命令：`RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`。logs保留于 `target/validation/linux-20261011-empty-producer/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。Windows原生补验仍因此前自动审批额度问题未执行，无绕过或Windows调用；无真实输入、设备、driver、安装或硬件操作。Linux/mock不证明Windows完成。
