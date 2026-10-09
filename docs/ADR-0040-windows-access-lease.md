# ADR-0040：活动会话桌面访问检查与短期租约

问题：startup 采样只说明当时上下文；进入高 integrity 窗口、安全桌面或访问失败后不能无限沿用。正常捕获不能周期性 GetAsyncKeyState，也不能在 Hook callback 查询桌面/token。

选择：新增不读取任何按键的 metadata-only check_input_access，callback 外检查 context pre/post 与所有 owned close。Active control 每 100ms 到期检查，成功后授予 500ms delivery-time lease；开始检查、结束检查以及 callback/actor tick 均验证租约，迟到结果不续约。等待包含下次 metadata 检查 deadline。启动最后的 IdleObserved 来自同样 context 检查，在空 binding/期限检查之后无 OS 调用地授予首次租约并启用。

查询不跨 actor/lease borrow。查询可能同步交付 callback，因此查询返回后先重验 native fault；已停止的 callback 不得被 control 再续约或 tick。checked_control 也保留并优先返回已有 callback fault，成功返回 plan 后不能继续 Peek/Wait。错误/租约失效终止会话，经既有 close-first 恢复；不自动提权、切桌面或恢复捕获。

替代：每事件桌面/token 查询违反 callback 边界；后台心跳不能证明本线程还在处理输入；周期性全 VK 扫描会破坏 capture 期间按键观察语义。

边界：metadata pre/post 仍非原子；100/500ms 是显式时间参数，不是墙钟实时保证。切换与下次检查之间的输入/SendInput 仍有访问竞态，此租约不证明目标应用收到输出。它不能发现 Hook 静默移除，不能替代独立进程进度监督、WTS/session disconnect 事件、安全桌面真机验收。所有新测试替换 Win32，Windows 编译/SDK/实际权限待验证。
