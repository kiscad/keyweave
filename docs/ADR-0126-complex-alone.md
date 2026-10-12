# ADR-0126：Complex actor 的 alone 分支

接入alone并解除其冷gate，held/delayed继续拒绝。actor保存最近一个有alone规则的momentary key Down候选；任何新key/button/consumer Down取消旧候选，变量事件不取消。Up不复活旧资格，非零wheel显式取消，零wheel/普通motion不取消。elapsed按保存实际Down与原Up交付时间，严格 < alone_timeout_ns（等于不触发）。

原main Up/逻辑scope/变量Up提交后，eligible alone使用原rule/Down modifier snapshot，在Up-time目的view/context一次filter并tapAll排队；随后after_key_up读取alone生成后的view。父等main/alone/afterUp所有child实际结束才释放；snapshot extra按原三方事务计树预算。halt在生成时压制分支，alone内halt也阻止后续afterUp。

wheel入口只控制alone资格与单调时钟，不处理原生pointer passthrough或modifier同步；完整controller负责同时间事件和backend。失败可在前一分支已生成后发生，沿用Failed/capture-stop/reset，不声称整次Up原子或OS恢复证明。Simple/scope/物理EOF/source池、held/delayed/context scheduler/native duration/Windows/硬件及原目标继续。

固定上游 `basic/basic.hpp:539` 的duration < timeout和`:808` 的momentary Down/非零wheel取消是本次来源；旧Rust Engine物理repeat/duplicate Down在cancel_alone前返回，变量不是momentary。新stage输入API仍不接physical Repeat，完整backend重复路径待接。

新增7tests覆盖timeout-1/等于/+1与alone tap在afterUp之前；另一设备button取消且其Up不恢复；wheel两轴/零值；变量输入不取消且fresh条件读其生成值；原shift/control已Up后snapshot临时owner重建、consumed shift隐藏且树预算1+1+2；alone变量影响afterUp并halt压制；旧rule跨reload和Reset不触发未生成alone。新增7及原20组合回放通过，无初始失败。

Linux工作区1176用例、Clippy -D warnings、fmt及source60通过：[验证记录](validation/linux-2026-10-12-complex-alone.json)。命令为 `RUSTUP_TOOLCHAIN=stable cargo test --workspace --all-targets --locked --offline`、`RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --all-targets --locked --offline -- -D warnings`、`RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`、`python3 tools/verify_sources.py`，logs在 `target/validation/linux-20261012-complex-alone/`。

无unsafe/FFI/deps/native ABI变化，未重跑ABI oracle。此前自动审批额度拒绝后Windows原生仍未补验，本轮无Windows尝试或绕过；无真实输入、设备、driver、安装或硬件操作。Linux/mock不证明Windows完成。
