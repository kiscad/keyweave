# ADR-0072：Linux 有限终止发送与观察

第三管道已有codec/单次IO，但真实入口不能无限flush或等待EOF。增加显式终止调度器：发送与退出后观察各固定100ms、最多64次IO尝试，显式MonoTime注入，IO前后与wait后检查单调性和期限，overflow/clock/wait/endpoint/预算均保留。完整frame写完或读取Complete不抹去随后发现的期限/clock错误；报告同时保存frame_sent/status与failure。Missing报告不写IO，不复制进度帧或增加epoch。

run_reporting_worker消费已在真实main核验的ReportingStartup与已准入Runtime，先原controlled report loop完成、Runtime释放捕获/输出，再独立有限发送。原worker outcome、termination事实和发送结果并列；父保护/进度端点重验失败在报告前拒绝，原stdout保留到真正exit。native调度器只借report FD调用既有CLOCK_MONOTONIC/ppoll，不新线程、不读progress、不运行Runtime timer。不会延长父watchdog lease；真实exe选择退出码/consumer-ready/bootstrap仍待接通。

Supervisor.collect_terminal只actual Exited后有限观察，第一次结果缓存；重复调用零IO/不重设deadline。旧poll_terminal保持手动步进兼容；用户host应选择collect_terminal并检查status、failure、原exitcode/强制原因和全部事实。收到了有效frame不等于消费者处理/全局恢复。固定budget是退出收尾策略，不能当成功保证。

测试需覆盖partial/WouldBlock与所有预算边界、完成后迟到clock/期限仍保留完成事实、无报告、failed发送不改原worker outcome、exit前零IO、native真实main发送/退出后有限收取与cache。全部不接触真实输入。

最终验证：Linux775 tests及扩展实际main无输入harness，Windows x64/x86各732 tests、Clippy/fmt/既有SDK通过，8 portable调度tests+3 Linux mock/nativefile tests。[报告](validation/windows-2026-10-10-linux-terminal-finalization.json)。初Clippy条件合并失败已改短路if-let，未放宽检查。真实capture与完整exe/host仍未接入，不把native synthetic frame当键盘恢复证明。
