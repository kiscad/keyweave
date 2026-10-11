# ADR-0121：保存 producer Down 的目的阶段选择

## 问题与选择

阶段控制器在 producer Down 实际交付时应使用该事件携带的旧 Arc、目的队列的生成环境与实际 delivered_at 选择一次。已有 GeneratedStageQueue.select 面向当前配置的新输入，不能把它拿来处理 reload 后旧父派生的迟交付事件；测试此前直接调用 select_rule 并手工提供flags，尚无保存完整producer决策的接口。

新增 ProducerDecision 保存完整输入 StageEvent、实际交付时间、目的stage和一次 RuleSelection（或pass-through）。GeneratedStageQueue.decide_producer_down 只读地验证Down、相邻输入stage、因果/队列时钟以及current或旧generation快照，按目的生成modifier/variable view和delivered_at上下文选择。OutputStageActor 包装相同接口。producer决策不改变队列、binding、vars或scope，不能授权旧root；提交仍需真实parent登记/激活的联合事务。

plan_main 始终用原Arc/device/source/delivered_at规划到目的stage。匹配但filtered to为空保持consume，不fallback。未匹配或变量事件pass-through保target/lazy，产生等待原Up的Down；incoming halt保留为originating_halt元数据，不把上游halt复制为新的下游rule halt。原RuleSelection保持可只读访问，分支在其生成事件再fresh filter，不在main重试时重选。

## 边界

Down输入receipt是调用方数据一致性，不是不可伪造的queue/native认证。决策构造虽允许旧快照，普通enqueue/prepare依然current Arc；旧计划必须经保留父/同事务激活证据准入。future/same-generation foreign Arc拒绝，旧schema校验保留；变量namespace准入仍由目的队列验证，Reset不复活旧名字。

这是完整controller要保存的producer状态，不拥有实际binding/来源/子树/原Up调度。完整controller还需联合准入、empty main父绑定、child originalUp/完成屏障、scope差分传播、分支/context等。Decision不含可变context或系统时钟；分支条件需调用方在正确生成时使用当前目的view判断。pass-through默认等待originalUp，不重播incoming tap/hold；producer自己的Up事件驱动保存的下游Up。生产duration gate继续保留。

## 验证

新增12个测试：旧事件Arc胜过当前配置且计划用actual delivered_at/decide不advance clock；冻结main indices保过变量变更和reload；未来时间lazy modifier已在目的生成视图但未实际交付；context到期按交付时间且unless Unknown不成立；未匹配lazy key pass-through保存原Up且halt仅元数据；变量pass保原assignment/key_up_value、不把dummy key重映射；matched empty永不fallback且变量变更不改旧决策；非法stage/Up/Repeat/receipt或目的clock倒退零状态；future/same-gen foreign/旧schema拒绝；非Clone决策作为实际producer payload联合Output准入并保存原Up跨reload；saved rule分支fresh filter不重选main；整数组先过滤再由第一个变量动作修改生成环境。

首轮focused12 tests通过，无初始失败；随后优化exact current Arc无需重复全schema扫描（已在new/activate校验），旧caller Arc仍完整验证。最终Linux工作区1132用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-11-producer-decisions.json)。

命令：`RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`。logs在 `target/validation/linux-20261011-producer-decisions/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。Windows原生补验仍因此前自动审批额度问题未执行，无绕过或Windows调用；无真实输入、设备、driver、安装或硬件操作，Linux/mock不是Windows证明。
