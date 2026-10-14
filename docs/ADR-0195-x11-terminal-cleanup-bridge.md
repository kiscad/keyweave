# ADR-0195：X11 cleanup 完成后的终端报告桥接

新增 finalize_x11_cleanup，仅接收 X11Cleanup.finish 生成的 X11CleanupReport；consumer_owner_dropped 字段改私有、只读 getter，不允许外部构造未完成报告。尚待退出的 X11Cleanup 没有发送接口，只能继续 step/finish，不因预算结束发布完成。

桥接复用 finalize_worker_report 的真实 pipe/nonblocking身份检查与有限发送，不修改既有终端协议或 Runtime termination facts。ReportedX11Cleanup 独立保 Runtime outcome、原取消/cleanup取消/poll/clock/timing错误和终端DeliveryReport，终端写失败不覆盖停机或回收事实。报告Missing termination时不伪造 Runtime 完成。

本次测试实际独占pipe成功发送/协议解码、普通File拒绝、缺Runtime termination拒绝；回收测试owner在发送前drop。没有session/display/input/uinput操作。图形 bootstrap/session host 接线后续完成，不能把本桥接成功当成 native provider/hardware验收。

全工作区发现 helper fixture 间歇性 ETXTBSY。该 errno 表示 exec 时文件仍被写打开（[execve 手册](https://man7.org/linux/man-pages/man2/execve.2.html)）；[Rust pre_exec](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html) 在 fork 后执行，FD继承使并行fixture writer成为可能原因（推断，未做 syscall trace确认）。测试在parent串行化helper创建/exec生命周期，unit helpers共用test-only锁；不在pre_exec获取锁，不修改生产spawn策略或重试生产失败。
