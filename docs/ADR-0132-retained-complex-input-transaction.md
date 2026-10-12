# ADR-0132：Simple 父绑定到 Complex 输入的联合事务

问题：GeneratedStageQueue 可以凭实际保留的 Simple 父绑定生成旧快照的 Complex 孩子，但 ComplexStageActor.prepare_input 只接 current Arc 的根。调用方不能安全地先提交生成，再尝试登记实际输入。

选择：新增 ComplexStageActor.prepare_generated_child_input 和 prepare_generated_empty_input。guard 独占上游父预备、Complex 目的生成队列和下游输入登记，全部预检通过后共同 commit。非空使用 Reservation；匹配空 main 使用 PreparedConsumedDown，不制造孩子身份。旧快照授权只来自 guard 持有的私有 ChildAdmission，普通根入口保持 current Arc 要求。两目的 owner 的当前 Arc 必须相同；孩子的 old Arc、身份槽数量、stage/source/device/start 必须精确匹配登记。仍检查实际已观察的 device generation、严格 batch ID 和源预算。

abort/drop 不能改变父激活、邻接边、变量、生成队列、输入元数据或 batch ID。分配失败/panic 不属于原子保证。commit 返回生成队列的原 Up 凭据，调用方仍须保留并提交保存 Up，排空实际孩子并退休生成批次后，才清理外部登记。

替代：放宽 public 根登记的 old Arc 会失去父绑定授权；先提交一侧会产生无法配对的半提交。因此不采用。

影响：提供 Simple owner 所需的真实旧父到新孩子接入路径；并非完成 Simple/allstage controller、scope 投影、全局来源预算或物理 EOF。外部 Simple arena 与内部 Complex arena 仍分别计数。native duration gate 保留；不进行真实输入或 Windows 构建。

验证：新增旧配置实际 Down/保存 Up/输出完成、两侧 abort 和准入拒绝、空消费与外来配置拒绝测试。具体命令和结果在验证记录中补充。

检查：`RUSTUP_TOOLCHAIN=stable cargo test --workspace --locked --offline`（1214 用例）、`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check` 和 `python3 tools/verify_sources.py`（60 hashes 与 fixture copies）通过。首轮 fixture 编译/API 用法与 old namespace 初始化错误已修正；未改变生产 namespace 约束。见 [验证记录](validation/linux-2026-10-12-retained-complex-input.json)。
