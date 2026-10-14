# ADR-0198：受控 X11 bootstrap 启动循环

新增 X11Bootstrap.run，nonblocking progress endpoints 预检后，共用 borrowed drive_until_ready：receive_startup_command先于每个step，standby不发布heartbeat、不读输入/写输出。Stop返回Stopped(原startup cleanup owner)，step/wait/协议错误返回Failure+同owner；成功进入原into_prepared，不重新创建bootstrap或clock。

Bootstrap.next_wakeup统一考虑全局预算、phase Verify旧证据expiry和平台probe deadline/poll interval；Native X11 pending poll2ms、一般housekeeping不改变证据时间。wait只监听独立progress控制端点并纳入该期限，fault收束现有捕获。驱动错误后close idempotent，retained平台未drop，可供同一child退出确认。

默认Console也复用borrowed控制算法，保原同步外部接口。host/session provider选择仍待连接，Native run仅编译；测试使用共用mixed平台、真实startup协议字节，验证Stop-first、捕获后取消、错误owner保留和期限选择，无session/display/device操作。
