# ADR-0191：准入事务内持续消费者检查

新增 CaptureGroup.try_arm_checked，接调用者的有界 check callback：冷校验后、各中立查询前后、每个 grab 前/后、最终整组确认后调用。CaptureDevice 私有 arm_checked 将 grab 后的 gate 故障纳入既有 RAII rollback；回滚失败仍关闭该输入，并保留 gate primary 与 release error。组继续回滚其余已捕获设备，任何 gate 错误关闭整组，不误报 Armed/WaitingNeutral。

默认 try_arm/arm 复用相同事务且 callback 为 no-op，现有行为保持；调用者仍负责独立监督及可信、无抓取副作用的消费者检查。此 callback 是连接 native X11 observer 的接线边界，不新增公有可伪造 ready token；未选择 host/worker 图形 scope，也未把 metadata 等同 driver/硬件证明。主动循环仍需独立续租。

检查发生在不可原子合并的 kernel 操作之间，不保证消除 ABA 或 syscall 内状态变化；失效后不再进行新 grab，并撤销已抓前缀。测试以实际共用事务和 mock source 观察调用/释放顺序，未打开真实输入或运行捕获。

新增 try_arm_x11 将私有 mint 的 NativeX11Observer observation 绑定 OutputNodeReady syspath，并在事务内每步读取实际时钟核验半开 TTL、单调时间；失效按相同 gate 回滚。测试构造器仅 cfg(test)，不提供生产 token mint。该入口只检查初始 observation，不替代独立监督、driver policy 和主动续租；host/worker 尚未选择。
