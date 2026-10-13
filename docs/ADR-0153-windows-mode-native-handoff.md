# ADR-0153：Windows 固定模式 host/worker/native factory 交付

Host check-windows/run-windows 显式选择 execution mode，默认 Legacy；冷 PreparedConfig.mode 生成固定 argv，四句柄 sealed image 仍只传原 bytes。worker 独立读取 inherited reader/legacy path 并以 argv mode 冷准备，显式关闭 reader 后把同 mode 传 Worker.install_with_mode → CaptureSession.install_with_mode → SessionActor.new_with_mode。原 install wrappers 保 Legacy。

模式在首次 probe/session/notification/Hook 前选定，所有旧 parent/job/进度/桌面/权限/中立/entry/卸钩/终止事实检查保留。Source-level staged 临时拒绝删除，因为现在实际传给对应工厂；不是以 Legacy 接受 staged argv。

共享 Rust 构造/CLI/参数采用 mock/offline 验证；Windows-only 工厂与真实 main 改动尚无 MSVC/native 测试，不能称平台 ready。不得启动真实捕获进行本轮验证。
