# ADR-0231：Windows 公开控制消息事务

Windows duplex named pipe 没有 Unix write-half-close，不能将 Linux request+EOF 协议直接搬来。增加独立的、无句柄所有权的服务端/客户端事务状态机，下一步由 native overlapped adapter 实现 transport 契约；本切片不宣称 Windows 公开端点已接通。

沿用 KWCC/v1 32-byte request 和 KWSC/v1 64-byte reply，每次连接只处理一条消息。服务端顺序为 Accept → Read → Authorize → Dispatch → Write → WaitClose → Disconnect → Accept。读取使用 65-byte 有界 scratch，并保留 MORE_DATA/消息长度事实，不能把截断当完整消息；完整鉴权与 decode 前不调用业务 handler。Dispatch 独立一轮且最多一次，冻结 reply 后不再次调用 handler；write 完成不证明客户端收到，更不证明 worker exit/recovery。客户端在匹配完整 response 后主动 close，服务端等待 close 或有界超时后取消/断开，不调用阻塞 FlushFileBuffers。

每 poll 至多一个 transport 方法或一次 handler，无内部重试。连接一旦 accept 完成即开始绝对 1 秒事务期限，客户端连接/授权/收发合用绝对期限；Pending 不延长。disconnect/close 额外最多 100ms，期限到仍未完成则进入 terminal failure，禁止复用 handle/内存；adapter 独占资源并负责有限清理/保留未确认飞行内存。时间回退、overflow、transport error、handler 返回无效或不匹配 reply 终止 machine，不假造清理成功；调用者仍必须显式关闭 adapter，Drop 只是回退。服务端请求处理结果与传输终止事实分开，允许 Stop 已执行而回复丢失。

Server transport 的 authorize 必须在该连接验证实际内核 peer/lifetime/主体，不能用任意 PID 或仅 DACL 取代；Client 同样验证实际服务端。业务 handler 必须有界，先 Supervisor 时限/exit 后服务 IPC。该纯 machine 不创建线程/管道/设备，也不发输入或续 worker lease。公开 trait 的第三方实现不因此获得可信证明。

下一步：接入 Windows FIRST_INSTANCE/REJECT_REMOTE/noninherit/SQOS/明确用户 DACL 的 native message pipe owner，确保 one-flight OVERLAPPED 完成后再 reset/free；继而复用 host Supervisor 和 CLI/GUI。生产 SDK/link、真实 Windows 无输入 pipe 对抗测试和硬件验收仍保留。

鉴权顺序依据 [ImpersonateNamedPipeClient](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-impersonatenamedpipeclient)：安全上下文来自最后已读取消息，故 Read 后 Authorize，不能仅依赖 ADR-0230 的 primary-token PID 比较。authorize 必须在每次返回（包括 Pending/error）前恢复原线程身份；恢复失败不能继续 host 工作，native adapter 必须保留独立失败事实。避免 FlushFileBuffers 的依据见微软 [overlapped server 示例](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-server-using-overlapped-i-o)。

验证：新增 11 项（10 个事务测试＋1 callback guard），Linux workspace 1960 项通过，8 native IPC 默认 ignored；格式、Clippy、源码 62、现有 Linux/X11 ABI 和 regex oracle 通过。Read→Authorize 顺序调整后，断开-before-request 测试的 terminal event 提前一轮，已修测试轮次并在 mock 强制最后消息上下文存在才可鉴权。没有执行 Windows 原生或输入。见 [验证记录](validation/linux-2026-10-16-windows-control-transactions.json)。
