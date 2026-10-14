# ADR-0187：原生 X11 消费者观察生命周期

新增 NativeX11Observer，由 WorkerParentGuard 创建实际 sd-login probe。begin 核验 owned UinputDevice/EvdevDevice 和 sysfs event 设备号、inhibited=0，再取得锚定 X11 会话，生成严格目标并启动唯一 owned helper。poll 保留同一 child 至实际 Exited；超时/取消/节点或状态异常清证据、请求停止，不重启任务。有效期 250ms 从 begin 首次时钟读取起算，包含节点、会话和 helper 耗时；完成后再次核验节点、inhibited 和会话，精确一致及覆盖才形成 observation。

此 observation 不是 capture grant。需求仍由调用者显式提供 X11 编号，未证明其涵盖输出 spec 全部 native codes；高分辨率投递、driver-specific mapping、SDK/live-server 验证及 bootstrap 授权接线尚待。不能据此单独选择抓取。helper 必须可信、不 fork；path 认证由 host 完成，现有 ProbeJob 的 executable 检查不代替认证。没有打开物理输入或执行捕获。

begin 在已有任务期间拒绝，即便任务已请求 kill；调用者持续 poll 或 cancel 后 poll 到 exit。每次 begin/poll 异常撤销 observation；私有 mint、非 Clone 证据绑定输出 syspath 和观察起点。单调时钟倒退拒绝；会话观察器的身份锚定持续保留。双快照不消除 ABA，active 不等于 unlocked。

clock source 失败后的已取消任务可用 poll_canceled，以保存时间只观察同一 child 退出，不延长或生成证据；未取消任务拒绝此路径。实际 owned helper fixture 验证该回收路径，不代表原生会话/设备验证。
