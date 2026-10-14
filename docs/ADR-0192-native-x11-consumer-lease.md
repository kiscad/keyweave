# ADR-0192：X11 异步消费者续租

新增 NativeX11Lease，独占 NativeX11Observer、实际 owned output EvdevDevice 和已移交 observation；NativeX11Observer.take_observation 移动私有证据，无 Clone/token 构造。首次 dispatch 前启动重新观察，后续在 observation 起点+100ms 续查。已有 lease 只在原250ms半开期限内可用，pending helper 时每2ms唤醒，期限取旧expiry与job deadline较早者；不把 pending 查询视作新许可。

check 先验证旧证据/单调时间，再 poll 唯一 job；成功新证据在真实查询后时间再次核验旧期限，不能跨空档恢复。新证据必须起点和终点都严格前进，并保持固定250ms窗口、同输出身份。unknown/report failure/时钟/query/output异常不可恢复地撤销 lease并cancel同一 child；保留 poll_canceled 回收路径。续租耗时计入有效期，不改变任一观察起点。

此适配器还未被 host/worker 默认选择，Runtime 主循环/图形 bootstrap 接线后续完成；它没有放宽 X11 driver policy、独立监督或 server/hardware 验证要求。测试以私有状态机和原有 owned job fixtures验证边界，不执行原生 session/display/input。

构造时从同一 observer 内部消费已完成 observation，不接受另一实例的 token；无已完成证据时 dispatch check 必定拒绝，但仍保留传入的 pending child/node owner供取消回收，不在构造失败分支直接丢 owner。
