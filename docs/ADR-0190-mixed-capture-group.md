# ADR-0190：显式混合输入组准入

新增 CaptureGroup.new_staged_relative，保留多设备冷校验、整组中立查询、逐设备 arm 和最终整组重验/回滚。显式 mixed scope 接纳 relative CaptureDevice 以及普通 keyboard peer，允许带按钮输出；默认构造器仍拒绝 relative source/pointing output。至少一个设备必须覆盖 emergency chord，纯鼠标组在任何 pressed query/grab 前拒绝。全部 input key/relative axis 都须由实际 OutputNodeReady spec 覆盖。

成功移交使用 Runtime.new_staged_relative，保留相同 staged keyboard 引擎与 mixed native pump；错误/中立等待沿用整组 rollback，失败先关闭输入。Console arm 入口明确拒绝 mixed group，不能借 console readiness 选图形 scope。通用 try_arm 的调用者仍必须事先取得独立 consumer/supervision 证据；此组不是新的 consumer grant，X11/native observer 授权接线仍待，host/worker 还不选择此 scope。

本次验证只用 probe/source/backend fixtures，未打开输入/uinput，未运行捕获。X11 driver 设置、原生 server、消费者有效期/丢失接线、Wayland 和硬件验收继续。
