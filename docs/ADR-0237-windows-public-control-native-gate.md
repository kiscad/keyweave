# ADR-0237：Windows 公开控制无输入验收 gate

新增 check_public_control 示例验收器与 check_host.ps1 -PublicControl 开关，使用已编译的实际 host/CLI 和固定 check_host_worker.exe，绝不接受 keyweave-worker.exe。fixture 新 public_control 模式从 epoch0 开始，只读原 private command，不 publish 任何 receipt；Stop 正常返回前 drop protocol client，不在拥有它时直接 exit0。

验收器冷创建自己的唯一目录/合成配置/日志，保留 host stdin 打开，不用 stdin/console 触发停止。读实际 host cold descriptor，实际 CLI 依次 Status Starting、错误 creation 实例拒绝、陈旧 public session 拒绝、再次 Status 未停止、有效 StopAccepted、最终 Exited。只以所持 Child handle 观察/终止自己的 host/CLI；实际 host report 要求 code0/Cooperative/资源完成/epoch0，且 sealed config 删除已观察。StopAccepted不作为actual exit，recovery仍false；所有报告都是无输入 fixture 证据，不是硬件恢复。

stdout/stderr 读取总量受限；每子进程与metadata等待有期限，失败/timeout不冒称清理完成，日志目录留存。不经 shell 拼接命令、不重用 PID 杀进程、不删除用户目录。fixture路径basename是误用防护，来源可信仍依赖选定验证快照/manifest，不是 executable attestation。

验收器的大部分 std/parser/进程编排代码在Linux可编译和测试；Linux入口明确exit78，不启动子进程。Windows专属fixture/creation_flags、SDK与完整原生链仍必须在Windows运行，不能由Linux绿灯替代。本切片只准备gate，不重试既有Windows原生审批额度拒绝。

验证：Linux全量1998通过，8 native默认ignored；格式/Clippy/源码62/Linux ABI/X11 SDK/regex oracle通过。独立 `cargo test -p keyweave-host --example check_public_control --locked --offline` 两项通过（完整descriptor/输出限额），Linux实际入口返回预期78，无host/worker子进程。初次cfg限定的mut lint已修。PowerShell与Windows fixture体未原生运行，验收器不是原生验收结果。见 [验证记录](validation/linux-2026-10-16-windows-control-gate.json)。

执行顺序：先以当前快照运行 tools/validate_windows.ps1 建立新的Windows-local ValidationRoot/source-manifest，成功编译x64/x86与SDK后运行 tools/check_host.ps1 -ValidationRoot <该root> -Arch x64或x86 -PublicControl。该switch同时保留原有host cases，并将新helper stdout/stderr与案例目录输出保留。复用旧root不会自动更新源码，必须核对manifest，不能将旧snapshot验收当新实现通过。
