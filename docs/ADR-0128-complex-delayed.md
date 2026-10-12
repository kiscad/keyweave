# ADR-0128：Complex actor 的 delayed invocation/cancellation

接入delayed并撤销剩余冷gate。一个pending delayed保存旧slot/checked deadline；原Up不撤销，父在Up和main结束后仍保Live直到任务处理完。新Down先撤销held并take旧delayed、生成Canceled旧snapshot tap及可能完成旧父，再用其生成view选择新输入。超时take任务并生成Invoked，条件读实际触发poll context/current目的view，modifier采用旧Down snapshot。变量Down同样取消，wheel只影响alone。

next_deadline合并output/held/delayed；due output先drain，两个timer都due时按最早deadline（同deadline held先），poll_with_context显式ctx。take保证一次性；finish producer额外等待pending delayed。device/reset丢任务，不能在退出时当canceled分支执行用户动作。

全Complex分支的生成/实际FIFO组合已接，但不是完整Simple/controller/native接入。所有先提交生成若后续准入失败均Failed/capture-stop/reset，无整事件回滚/OS接受证明。source ID/全stage scope/物理EOF/context scheduler/simultaneous/native duration/Windows/hardware/GUI/packaging/driver等原目标继续。

固定来源 `basic/to_delayed_action.hpp:94` 仅Down setup，`:121` 仅新Down cancel且post canceled，`:143` scope snapshot，保存shared original event直到post结束；原Up不取消。

新增9tests：原Up后父/任务继续直到Invoked tap完成；Canceled变量先影响新Down条件选择；timer fresh context与旧rule跨reload；held/delayed同deadline顺序与generation halt；empty at最大batch ID直接finish；设备/Reset丢任务不触发用户分支；deadline overflow零父/输出提交；原modifiers/原Up都结束后snapshot重建/extra预算；变量Down cancel而wheel不cancel。原36与最终45组合回放通过，无初始测试失败；首轮all variants覆盖后有冗余wildcard unreachable warning，删除冗余匹配，最终Clippy clean。

Linux工作区1194用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-delayed.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-complex-delayed/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝后Windows原生仍未补验，本轮无Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作。Linux/mock不是Windows证明。
