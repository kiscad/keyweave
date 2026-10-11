# ADR-0112：可组合的阶段队列预备事务

## 问题与选择

完整阶段执行器需要同时准入 GeneratedStageQueue 的生成视图与实际执行队列。先提交生成变量/作用域，再准入执行队列会在后者拒绝时留下不可用 cancel 回滚的生成状态。采用独占借用的预备 guard：全部检查和固定 FIFO 时间位移在 prepare 完成，commit 消费 guard 提交，丢弃 guard 则不改变队列、变量、交付预留或作用域。

StageTimeline 提供 prepare_enqueue，预备结果拥有原 events 和 effective times，并独占借用原时间线；abort 返回未修改的原 events。GeneratedStageQueue 提供 prepare_enqueue 和 prepare_scoped，组合环境 overlay、交付账本和时间线 guard；scoped original-Up 凭据只在 commit 返回。原 enqueue 方法继续作为 prepare+commit 的便利包装。

不用先写后撤销：变量赋值和逻辑作用域不能由取消未执行事件恢复。也不引入共享锁、unsafe 或跨线程事务。commit 无可恢复的业务拒绝，但普通 Rust 分配失败/进程 panic 不在原子保证内。独占借用防止准备之后控制或 reload 改写校验依据；两个独立队列可分别准备后再提交。

## 边界

本步是组合基础，不是完整阶段 actor。TerminalActor 尚未暴露预备事务，scoped original-Up 仍使用现有单队列原子路径；跨两 actor 的完整准入、scope 差分传播、保存父子绑定和统一来源预算仍须实现。不得解除生产 UnsupportedActionTiming gate。Windows 编译补验仍待，WSL/Linux 成功不替代 Windows 或物理键盘验收。

## 验证

新增8个集成测试：时间线 abort 返回无 Clone/Debug payload 与原时间/旧 Arc/source 且保 clock；第二队列时间位移溢出丢弃第一准入、原批次重试保50ns跨度；独立队列成功 prepare 后 commit 保同刻因果顺序；variable tap abort 保变量为空及交付身份无幽灵，重试保 Up 最终值；第二生成队列缺少 device 拒绝不改变任一逻辑视图，重新准入后可联合提交；persistent consumed scope abort 不隐藏旧 owner/保原ID；第二 scoped 队列 retired device 拒绝不隐藏第一，fresh destination 联合成功返回各自一次Up凭据；snapshot variable scope abort 不赋值或预留额外来源，成功保原预算。

首次新增测试编译出现测试代码的错误枚举名和重复 unwrap；已修正使用既有 RuleBranch 和 Option API，未改业务实现迁就测试。focused tests 通过；随后 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline` 共1033个用例通过，`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check`、`python3 tools/verify_sources.py`（60个上游文件及fixtures）通过。无unsafe/FFI/依赖/ABI变化，ABI oracle未重复运行；未进行真实输入、设备或驱动操作。

[验证记录](validation/linux-2026-10-11-prepared-stage-admission.json)。
