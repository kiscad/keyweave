# ADR-0069：Linux 显式协作停止

用户态 Linux host 需要给 worker 有限收尾机会，不能用 signal acceptance 代替实际退出。沿用共享 KWPS/v1 StopRequest，Linux ProgressClient 增加显式 receive_command；旧 receive/run_worker 默认继续只接受 challenge。新增 run_worker_controlled 对停止优先：在普通 poll/due 前 pause 关闭全部捕获，再只做原有有限恢复，无额外 publish/wait/read；恢复错误保留，返回 StopReason 仅表示循环结束，不证明全局键恢复。原 startup/channel/父保护要求不变。

Supervisor.request_shutdown 仅用于已知支持该协议的 worker。期限为 min(旧 watchdog lease, now+2s)，重复请求不延长；已有硬故障保持。未尝试挑战可替换，尝试过的完整或部分 frame 必须先完成，再下一连续 id Stop；每 poll 最多一次 Write，停止后不 Read/observe/续租。request_sent 仅指 stop bytes 写完，Cooperative 仅为请求意图；只有实际 try_wait 返回才 Exited，退出码独立。时钟倒退、期限/identity耗尽、写错误强制终止自有 Child，旧 request_stop/Drop 继续有限紧急回退。

控制模式的等待应在 ticket/partial reply 未完成时仍监听 reader，以使 Stop 不依赖先确认 progress；使用同一 borrowed FD，不复制设备或新建线程。旧 wait interest 保持。共享 codec 没有改格式或 Linux 默认协议，不能假设所有旧 worker 支持 Stop。

验收使用 mock Runtime 与自有无输入 fixture child：停止先于物理读取与 held 到期、关闭捕获先于输出、partial reply 可停止与 pending ticket 的控制等待、畸形帧/恢复错误，parent 未尝试/已尝试 challenge、旧 lease/重复请求、时钟与耗尽、强制停止覆盖协作、实际成功或超时退出。真实 Linux bootstrap/consumer ready/hotplug/add 与硬件验收仍待实现。

最终验证：Linux748 tests及独立无设备harness，新增6 mock worker/wait与6 owned无输入child tests；Windows x64/x86各710 tests、Clippy/fmt/既有SDK通过，Linux专有case在Windows排除。[报告](validation/windows-2026-10-10-linux-stop.json)。初次短恢复错误预期已修正为flush后成功，另增UnknownAcceptance错误保留测试，生产契约未放宽。
