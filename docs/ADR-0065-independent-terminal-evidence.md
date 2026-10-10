# ADR-0065：Windows 原生退出码携带独立终止事实

ADR-0064的标量Clean/Loop/Cleanup丢失TerminalFacts已独立检查的结果。新增TerminalEvidence：固定高16位0x4b57，低10位依次为fault/progress_failed/shutdown_missing/stopped_unconfirmed/unhook_unconfirmed/recovery_unconfirmed/notifications_unconfirmed/access_lost/session_changed/untracked_input。保留全部同时存在的证据，不按主故障选择其中一个。未知reserved位、零mask/tag、其他tag严格拒绝，不能按低byte冒充旧code。完整退出码属于Windows契约，Unix不得截断复用。

终止后clean仍0，早期64–69/78保持；真实worker拥有完整report与shutdown时，用TerminalFacts.native_exit生成非零终止事实，显式drop(worker)且execute返回清理完局部owner后，main才完成进程退出。Windows std ExitCode的u32构造在固定1.97.1仍是unstable；高位已知值调用stable std::process::exit(i32)，tag保证在正i32范围，普通code返回ExitCode。不得在仍持worker/pipe/artifact时直接调用exit，因为不运行stack Drop。依据：[Rust process::exit](https://doc.rust-lang.org/std/process/fn.exit.html)、[ExitCodeExt](https://doc.rust-lang.org/std/os/windows/process/trait.ExitCodeExt.html)及本地1.97.1 rustdoc。

host只在实际owned Exited后精确decode，显示静态terminal issue列表，保留原码/监督原因/资源报告。cleanup类别优先决定建议，fault/progress仍列出；未知格式仍UNKNOWN，clean不覆盖独立reason。不存在report/shutdown的早期返回仍保留原Loop/Cleanup，不能编造哪些API失败。notification位是原完整cleanup != default的聚合，不是假定具体注册/窗口/class错误。此mask不含键码/路径/marker/config/API字符串，不认证image、不证明global恢复，也不是完整详细错误通道。

无输入验收：枚举全部1024个合法mask组合（零为clean），每个bit与组合可逆、reserved/tag/alias拒绝、cleanup优先而证据保留；Windows自有子进程返回完整mask由真实Command/Job句柄观察，host无输入fixture显示同mask及独立supervisor结果。fixture只合成事实，不能代替CaptureSession/Hook/SendInput/真机。

最终验证：Linux708、Windows各682tests；每arch7个真实host无输入场景、Clippy/fmt/SDK与195源hash核验通过。所有1024个纯mask组合及native全宽返回均验证，fixture仍只合成事实。[证据与失败保留](validation/windows-2026-10-10-terminal-evidence.json)。
