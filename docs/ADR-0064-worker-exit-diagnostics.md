# ADR-0064：实际 worker 退出码的可操作诊断

现有 worker 在真实 main 中返回固定整数分类，host 只显示数字。补齐共享 WorkerExit::from_code 与只含静态文本的 ExitDiagnostic：稳定 code、含义、下一步建议；未知 u32 保留原数值与明确未知分类。不得解析配置/记录按键、进程 PID/路径/marker，不增加 IPC 或改变进度帧。

分类只有在 host 已实际观察 owned process Exited 后才可用于报告；None 是未观察退出，不冒充 clean。Report 仍保留原 code、独立 TerminationReason、资源清理与 diagnostics，成功判定完全不变。退出码来源并未认证：开发 image 也可返回相同数值，host 只按已知 worker 契约解释；强制终止或异常退出可返回未知值，不能把分类当作详细因果、主故障或 ledger 恢复证明。

诊断特别保守：Config 涵盖读/大小/UTF8/编译/无损准入，不声称某字段有错；Install 涵盖输出/Engine/访问/通知/Hook 启动，不声称具体 API；Loop 不证明实际输出恢复；Cleanup 包括独立收尾错误/Unresolved/访问或session变化，不能靠重启擦掉；Clean 也不是全局按键状态证明。完整逐错误独立终止报告与 GUI/控制面仍需后续通道设计，不能把标量说明当作完成。

验证：跨平台覆盖完整已知集合、未知值（含259、forced值、u32::MAX）、静态隐私文本与 TerminalFacts 独立故障分类；Windows Report 测试 None/已知/未知，以及 classification 不改变成功判定/不抹掉独立reason与cleanup错误。真实无输入 worker 缺配置和无效封存bytes路径继续覆盖 code66；不以有效配置启动capture。

最终验证：Linux705、Windows x64/i686各678tests及两架构各5个真实host无输入场景通过；source194核验匹配，Clippy/fmt/SDK通过。完整原始证据与限制见[JSON报告](validation/windows-2026-10-10-exit-diagnostics.json)。
