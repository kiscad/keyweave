# ADR-0119：旧 Simple 父到 Complex 生成队列的联合准入

## 问题与选择

ADR-0117 已为旧 Complex 父到 Output 提供三方事务。保存的 Simple 父在 reload 后向 Complex 生成新孩子时，普通 GeneratedStageQueue.prepare_scoped 的 current Arc 限制仍会拒绝原配置。不能为了这个路径放开普通 root，也不能先修改生成视图再登记父子边。

GeneratedStageQueue.prepare_complex_child_scoped 消费上游 child Reservation，仅供目的 Complex 阶段。它使用既有 crate-private ChildAdmission 验证 origin/Down槽数量并授权 current 或更旧 Arc，自动计入 scope 额外来源，再预备逻辑视图、交付预算和时间线。PreparedGeneratedChild 同时持有登记与生成 guard；commit 登记父子边、提交生成状态并返回原 Up 的 ScopedGeneration；abort/drop/错误不产生登记、scope、变量、队列或凭据副作用。

## 边界

父必须是同一 arena 内实际 Live 的 Simple slot，设备/source/旧 Arc 一致，且是严格下一 stage；Reserved 父、无父登记、future/same-generation foreign Arc、任意 origin/槽数错配均拒绝。普通 root API 保持 current Arc 检查。Output 仍必须使用 OutputStageActor 的生成/实际执行/上游登记三方路径，本 API 拒绝 Output/Simple 目的 queue，避免把仅生成事务误当 Output 执行准入。

这仅完成 Simple→Complex 的联合生成准入，不执行 downstream mapping 或实际 producer binding。登记 child 的槽位仍 Reserved，交付 Down 后应由完整 controller 插入实际绑定；生成 Up 或队列退休并不解除父子阻挡，必须完成实际孩子/scopes 并清理外部登记。额外来源计到整批登记清理；上游父含义由库调用方负责，不是 OS 认证或恢复证明。完整 producer/controller、跨stage scope差分、全局 source ID 分配、context/分支/native duration/simultaneous/UI/包装/Windows补验/硬件继续，生产 duration gate 保留。

## 验证

新增10个集成回放测试：旧Simple父跨reload的Complex Down/原Up、preparation abort/drop后同计划重试且modifier/变量/隐藏引用不变、所有origin字段/槽数错配、普通old root/future/same-gen foreign拒绝、snapshot来源overflow与精确16384直到外部登记清理、8192满队列拒绝后同child重试、目的stage/parentless/Reserved父限制、Reset变量namespace不复活、设备remove丢未来事件但不代外部清树，以及手动旧Simple→Complex→Output树组合和原Up孩子先父的清理。该组合测试手动驱动交付和arena.insert，不是生产controller或跨stage modifier传播验收。

首轮7通过/3失败：两个测试错误地要求last repeat=true主目标等待hold duration，另一个把旧变量名Reset后准入错误写为Lifecycle而非InvalidVariable。核对固定上游 `basic/event_sender.hpp:225` 与原planner，修正测试断言；第二轮8通过/2失败是测试在原Up admission已经advance到110后poll109，调整no-due断言到原Up之前。第三轮10通过，生产时序/namespace错误行为保持原实现。

Linux工作区1109用例、Clippy -D warnings、fmt与source60通过：[验证记录](validation/linux-2026-10-11-complex-child-generation.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`；logs在 `target/validation/linux-20261011-complex-child-generation/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。Windows仍因此前自动审批额度问题未执行补验，没有绕过或Windows调用；没有真实输入/设备/driver/安装或硬件操作，Linux/mock不能证明Windows完成。
