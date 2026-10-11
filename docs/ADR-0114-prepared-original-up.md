# ADR-0114：保存原输入 Up 的预备事务

## 问题与选择

Down/初始批次已有两侧预备事务，原输入 Up 仍立即修改生成作用域和变量。生成侧成功后执行侧因 DeadlinePending/满队列拒绝，会造成只关闭一侧。增加 GeneratedStageQueue.prepare_input_up_scoped 与 TerminalActor.prepare_input_up，返回可消费的预备 guard；已有无操作情况返回 None，错误保原类型。两侧全部准备成功后 commit，否则丢弃 guard 不更新任何 Up 标记、逻辑scope、变量、交付预留、队列或时钟。

StageTimeline.prepare_enqueue_at 在准备时核验单调时间，成功提交同时追加 events 和推进已验证时钟；abort不推进。保留原 enqueue 后 advance 的顺序语义，预备追加时间仍按原FIFO尾/当前时间计算，已排队earliest不重写。原 input_up 方法包装新 prepare+commit。

Generated guard独占 queue字段与ScopedGeneration，持有旧plan产生的Up及原queue身份。Terminal guard独占原Batch和时间线，保due先drain、scope最后modifier的私有ScopeUp与oldArc。预备不释放实际owner；实际poll仍以保存Bound关闭，并保持future duration。

## 边界

调用方仍须确保两侧同一批次和一次Up状态一致，处理一侧None/另一侧Some的失配；本步不以两个可组合guard代替完整stage actor。控制、设备取消、旧Arc、预算和duration gate不变。commit无新业务检查，分配失败/panic不在原子保证内。Windows原生补验、完整stage/交付退休协调/scope传播/统一来源预算及原目标继续。

## 验证

新增8个集成测试：时间线clocked append abort不改clock、commit保旧earliest20但实际delivered100；时间倒退/整体位移溢出返回原events并保clock；两侧原Up准备200后abort仍可150重试，scope/slot/upGenerated保原值，完成后duplicate None；Terminal DeadlinePending拒绝不关闭生成scope，先drain后同刻重试；Terminal精确8192满队列拒绝保生成recipe/scope，取消自有pending释放容量后原Up重试且实际Up仍遵FIFO；反向Generated8192满拒绝丢Terminal marker与clock，110abort后105重试，两侧生成关闭但晚delivery独立；oldArc变量Up200abort保值1，reload空generation2后110commit生成值2、实际poll后值2，交付event仍旧Arc；repeat=false最后modifier全部KeyUp已交付仍需原Up，预备abort不结束scope，成功仅Terminal排1个ScopeUp/生成不造KeyUp，实际恢复shift后退休。

focused tests（包含既有generated scopes/terminal actor）通过，无首次新增测试失败。随后 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline` 共1050个用例通过，`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check`、`python3 tools/verify_sources.py`（60文件及fixture）通过。无unsafe/FFI/依赖/ABI变化，未重复ABI oracle。只运行纯引擎及已有无输入检查，无Windows原生、真实键盘、设备、安装或驱动操作。

[验证记录](validation/linux-2026-10-11-prepared-original-up.json)。
