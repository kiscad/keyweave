# ADR-0241：设置界面的 Linux 只读设备发现

问题：已有只读 evdev CLI，但配置设备选择器仍需手工填写。设置界面应能显式刷新并从观察结果生成草稿，保持 UI 响应和已有未提交编辑。

选择：Linux 设置界面增加独立 metadata 面板。显式 Refresh 启动单后台任务，主线程仅在 is_finished 后 join；进程级共享 busy guard 防止关闭/重开草稿时叠加扫描线程。刷新先清旧结果，不自动周期扫描；任务不能中断同步 syscall，关闭面板不宣称扫描已经结束。所有设备访问复用 ADR-0240 inspect，不启动 worker/grab/uinput。

快照显示固定来源 /dev/input、年龄、scan/probe 完整性与逐节点错误；Unknown 类型保留未知，禁止据此生成 false 类型。选择必须来自当前完成结果、开始观察后30秒内（时钟倒退也拒绝），不把时间门槛当热插拔身份保证。刷新中与无结果时不可选择。已知节点在部分快照中仍可生成草稿，但不称完整库存。只复制 vendor/product/两种分类到新的 DeviceEdit，保留其显式 Commit/Discard 及 append fence；有未提交设备编辑时拒绝替换。路径、名字不进入配置匹配身份，具有同一选择器的设备会共同匹配。

替代：UI 同步扫描会阻塞帧；每次重建面板启动线程会累积阻塞任务；从键码猜分类会错误消除 Unknown；直接改 JSON 会绕过草稿确认。均不采用。

影响：仅 Linux 元数据发现；Windows discovery、runtime Start/Resume/Apply、热插拔与真实桌面/硬件验收仍待实现。GUI 中生成选择器不是 capture readiness，也不更新 Runtime inventory/conditions。默认测试使用合成快照与受控扫描任务，不打开物理设备。

验证：新增6项合成测试覆盖已知/Unknown/不可用节点、年龄/时钟、刷新失效旧快照、活跃任务轮询、关闭后共享 guard、任务 panic 后恢复、生成选择器草稿与 append fence，以及 headless 布局。Linux workspace 2021通过，8项既有 native IPC 默认忽略；格式、Clippy、62上游源码 hash、regex oracle、Linux C ABI 和 X11 SDK 共七项检查通过。初轮使用旧 egui Context.run API 的测试编译错误已修正为 run_ui。未运行原生窗口/设备，不能据此宣称硬件发现验收。见 [验证记录](validation/linux-2026-10-16-settings-inventory.json)。

后续：Windows 原生补验与设备发现、GUI 运行会话 Start/Resume/Apply/端点发现、设备热插拔和真实输入恢复验收仍是完整交付要求。
