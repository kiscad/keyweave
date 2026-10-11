# ADR-0116：输出阶段保存绑定的物理 Repeat 目标

## 问题与选择

现有 Windows runtime 只在物理Repeat后查询原Engine保留的叶绑定，并要求output desired/submitted都持有同键，再提交Repeat；Engine.handle本身不创建Repeat owner。OutputStageActor重构后需要同样只读的目标查询，不能重新匹配配置、重放tap/变量/宏或自行启动重复计时器。

新增 TerminalActor/OutputStageActor.physical_repeat_target(BatchId)，从尚未排队原Up的批次第一条saved-Up slot取得实际Live Bound，复用Engine原physical_repeat_target的保留叶检查：keyboard非modifier、实际emitted、精确source仍在owner集合、未hidden。Reserved/Released、scheduled tap、repeat=false、变量/consumer/button/modifier、已Up排队、取消/移除/Reset/失败都返回None。第一saved-Up指主目标，其后的scope modifiers不能成为repeat目标。

BindingArena.live只读返回Live payload，不Clone、不改变生命周期。Engine将原leaf检查提取为共享私有helper，递归旧rule路径不变；query不推进时钟、queue、source或预留，也不产生OutputIntent。原Up刚准入即停止物理重复，即使actual Up因FIFO尚未执行，避免迟到的physical repeat沿用已释放来源。

## 证据与边界

本地event_sender.hpp按filtered to数组的最后eligible action与repeat决定立即Up或保存原Up（现有source manifest已核验）；只补保留目标查询，不增加Mac重复计时推断。Windows runtime的desired/submitted、own injection和当前session证据仍必须独立核验；查询结果不是OS已接受证据。Linux仍Kernel repeat，不能同时引入physical/external repeat源。

OutputStageActor仍是Output目的stage，native runtime和完整Simple/Complex父绑定重复目标传播尚未接入；生产duration gate保留。旧Arc、来源身份与两侧准入/退休语义保持。Windows补验、scope跨stage传播/旧父新孩子/source子树预算/context/分支/simultaneous/硬件及原目标继续。

## 验证

新增8个OutputStageActor集成测试：prefix b在50ns期间按住不能重复，最后c实际Down后10000次只读查询无queue/owner/预留变更；原Up因其他future tap延到1050仍在准入10立刻失去repeat资格，future tap不repeat；同键不同批次/设备原source独立，释放一个不借另一个owner；reload保旧b绑定，新batch新z且unknown ID无目标；modifier/variable/consumer/button/repeat=false不重复；附加modifier先Down时main Reserved无目标，主b Down后只repeat b，查询不重建shortcut；设备generation替换及Reset撤销旧ID；selected consumed scope的主b只读查询不恢复旧修饰键，原Up后即失效。

新增1个BindingArena非Clone/Debug payload只读生命周期测试，覆盖Reserved/unknown/out-of-range/Live/Released/canceled以及1000次读取不改变预算；1个Terminal私有状态注入测试证明其他source持有同键不够、hidden或未emitted均无目标，精确可见emitted原owner才可repeat，Failed拒绝；原Output失配Failed测试补查询None断言。既有5个递归Engine physical_repeat测试全部通过。

focused tests通过，无首次新增测试失败。随后 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline` 共1076个用例通过，`cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`cargo fmt --all --check`、`python3 tools/verify_sources.py`（60文件及fixture）通过。无unsafe/FFI/依赖/native ABI变化，未重复ABI oracle。只运行纯状态与已有无输入检查，无Windows原生/真实键盘/设备/安装或driver操作。

[验证记录](validation/linux-2026-10-11-output-repeat.json)。
