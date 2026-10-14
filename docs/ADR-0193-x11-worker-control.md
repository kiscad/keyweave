# ADR-0193：X11 lease 接入 worker 控制循环

新增显式 run_x11_worker_controlled_report，以 NativeX11Lease 在每次 Active Runtime.poll 前检查消费者，并将续租/探针期限交给现有 native reactor。Stop 命令仍先于 provider IO，lease failure 保独立 WorkerError.X11Consumer，走 ControlFault input-close-first/recovery，不再读下一输入。

将既有控制循环分为 owned wrapper 与 borrowed controller 实现，保同一调度/停机算法。borrowed handoff 在 Runtime owner drop 后取消 lease，返回 X11WorkerReport 的 worker facts、独立 cancel_error 和仍持有的 consumer。尚有 probe 时调用者持续 consumer.poll_canceled 到实际 Exited后，才能完成 provider cleanup；不会把 Drop/kill 请求报告成 child 已退出。guard/FD preflight 失败也先收束 Runtime并归还 consumer owner。

默认 Console/host/worker provider 选择不变。此入口等待图形 bootstrap 使用；测试复用生产 X11Control 和同一 Runtime worker loop，通过私有 lease mock 检查真实派发/恢复顺序，而非声称 native session/display已运行。终端报告桥接仍需处理归还的 probe owner。
