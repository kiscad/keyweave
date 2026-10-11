# ADR-0115：单一拥有者的输出阶段 actor

## 问题与选择

独立 GeneratedStageQueue 与 TerminalActor 已具备初始批次和原Up预备事务，但调用方还要保存正确的queue凭据、匹配同一BatchId、交付对应事件、区分scope marker、实际结束后退休，以及同时控制两侧。增加 OutputStageActor，私有持有两组件及每个非空批次的ScopedGeneration/device，不暴露可变组件。raw计划使用内部无consumed/snapshot的中性scope，映射计划保原frozen scope。

初始批次与原Up两侧先prepare再commit；未知/空/已完成Up no-op，已存在recipe要求两侧Some/None一致。terminal每次实际poll提供crate-private交付receipt，actor只为普通action从生成队列取一个event并核对完整slot/edge/target/lazy/halt/device/source/旧Arc/earliest，ScopeUp只推进生成clock而不消费别的批次事件。Terminal报告实际batch完成后才退休生成账本并删除recipe。空计划推进原严格BatchId但不留recipe。

只读selector使用生成modifier/variable视图；实际pressed_outputs及executed_variables单独查询。设备控制、cancel、Reset同时对两侧处理，实际owner清理只沿Terminal保存绑定，未执行Down没有Up。reload两侧先完成 generation/schema/历史变量并集检查，再提交同一Arc；原queued工作和savedUp保旧Arc。

内部不一致/实际执行失败锁存Failed，普通匹配/enqueue/Up/poll/reload停止，控制/Reset仍可恢复。失配发生在实际执行之后可能已有未交付intent，错误不承诺回滚；runtime必须按原实际output ledger进入恢复，Reset返回引擎已记emitted的Up不是OS接受证明。

## 边界

这是Output目的stage的可执行组合，不是完整物理Simple/Complex actor。外部仍分配严格单调BatchId/source、生成already-filtered计划和协调parent halt；跨stage scope传播、旧父生成新孩子、源子树预算、context provider与extra branch调度仍待。只有私有同actor路径保证两队列时钟与事件交付相同，不能把旧独立fixture的任意两队列当生产组合。native UnsupportedActionTiming gate保留。

无OS、线程、IPC、实时时钟、unsafe或新依赖。普通有界分配失败/panic不在prepare/commit业务原子保证内。Windows原生补验和真实键盘/设备/GUI/安装/driver及原完整目标继续。

## 验证

新增12个集成测试：raw bDown/50ns后bUp+cDown/原Up60后cUp自动退休两侧；生成consumed视图立即影响selector，实际scope到poll才开；repeat=false末modifier原Up只有ScopeUp，同刻marker不消费后续c批次；snapshot变量生成值2/实际Down值1/Up值2并回收额外来源；pending取消不动原physical shift；移除mapped设备释放其实际ctrl并保另一设备shift、拒绝旧generation/re-arm新generation；失败reload保两侧原Arc，成功空generation2仍原变量Up；精确8192队列满variable准入失败无赋值，取消pending后原ID成功，late5000回放全部退休；Terminal独有ScopeUp在满队列拒绝保生成scope，105控制释放后110同recipe成功；empty无recipe/Reset保严格ID；due先drain/倒退不关闭scope；同键两来源独立取消和Up。

4个私有状态注入测试不暴露生产mutable组件：缺生成receipt导致实际Down后Invariant并latch普通操作，Reset返回实际emitted Up再恢复；一侧已Up/另一侧pending拒绝并丢Terminal预备，Reset协调清理；第二侧reload generation拒绝不提交第一Arc；第二侧device generation拒绝不退休第一history。

第一次focused测试将带空格的变量名误判为非法；既有语义允许空格，测试已用空名称触发真实schema错误，未改变compiler/IR规则。修正后focused tests通过；随后 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline` 共1066个用例通过，`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check`、`python3 tools/verify_sources.py`（60文件及fixture）通过。无unsafe/FFI/依赖/native ABI变化，未重复ABI oracle；无Windows原生、真实输入、设备、安装或driver操作。

[验证记录](validation/linux-2026-10-11-owned-output-stage.json)。
