# ADR-0047：共享进度协议与 Windows 实际 poll 计数

Linux 已有固定 24-byte challenge / 40-byte receipt、lease/watchdog 和 single-flight client，协议代码却依赖 Linux RuntimePhase 与 Runtime。Windows 独立监督需要同一行为，不能复制校验器或后台心跳。types/engine 继续不依赖 IPC。

选择：新 keyweave-progress crate 拥有现有 wire version / frame / policy / watchdog 与通用非阻塞 Read/Write client，显式 epoch 和 ProgressPhase，不接触 OS/时间/线程/设备。Linux RuntimePhase 作为同一共享 enum 的别名，原 ProgressClient 保留 Runtime 类型接口，仅适配 epoch/phase 与 ppoll FD interests；Linux worker/supervisor/父保护不变。wire bytes 和阶段数值保持，原协议 tests 随代码迁移，不删 oracle。

Windows ProgressClient 包装通用 client，仅接受 CaptureSession 的只读快照；constructor/receive/publish/flush 在 keyboard callback 中均拒绝，不做IPC或借actor。CaptureSession 私有 progress_epoch 仅在实际 poll 协调器完成并处理 shutdown 后增加，不从 key callback/tick/config/metadata/后台线程或手动 shutdown 增加。Standby 不 publish，Active 对应 Active，StopRequired 对应 Recovering，Stopped/Unresolved 对应 Stopped；后者只表示循环停止，不证明键已释放或进程退出。counter 耗尽先停 Hook/恢复并返回错误，不绕回旧身份。

client receive 完整有效请求时取 baseline，publish 必须看到其后的实际 poll；partial/WouldBlock 保留同一 receipt，不靠 flush 造进度，最多一条请求/回复在途。endpoint 必须真实非阻塞；通用 Read/Write trait 无法证明 OS flag，平台启动事务继续负责。run ID/private-channel 鉴权与可信 worker 持有 runtime 是 host 的职责，协议不是抵抗恶意 worker 的认证系统。

这是监督协议和 Windows loop 接入切片，尚无 Windows native pipe/overlapped transport/Job Object/父死亡保护/进程监督或 executable host；不能据此声称已有完整 watchdog。后续 supervisor 必须独占已创建 Child，实际 loop 完成响应，超时只终止该 Child，signal 与 exit proof 分开。全部平台硬件/Hook liveness/Windows fault acceptance 仍待执行。
