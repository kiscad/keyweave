# ADR-0234：Windows 控制客户端实例 pin 与 transport

实现 ADR-0231 ClientTransport，复用原绝对 1s connect/auth/write/read 和额外 100ms close 期限。客户端冷准备要求显式 endpoint 与预期 ControlServerInstance(pid, creation FILETIME)，先打开 limited-query＋SYNCHRONIZE、非继承进程句柄并验证 PID/creation/存活/完整 SID-logon-session，再打开管道；在实际 client handle 上查询 kernel server PID 与所持实例比较，收发前后复核，不重新按 PID 查找替代进程。instance.current 从 host 当前进程生成，parts 构造只是公开实例提示，不是认证凭据；后续 host 发布/发现必须明确来源与陈旧实例处理。

连接每 poll 至多一次 CreateFile；仅 NOT_FOUND/PIPE_BUSY 且工厂回收明确成功时 Pending，无 WaitNamedPipe、后台重试或自启动 host。其他失败终止。鉴权前不发 request，不接受未完成/不匹配回复；reply 仍不证明 worker exit/recovery。server pin 保持到 pipe close 确认后，独立关闭错误保留；close/pending/Drop 不能伪造完成。callback 内不做 native I/O，未知 pending Drop 保留 pin/flight 到进程 exit。

这是 per-user/logon/session 的实例约束，不是签名/image attestation、跨用户 broker 或对同主体故意代理/句柄转交的证明；预期 metadata 不是秘密，不能从任意对端自报值推导授权。实际 creator-PID/转交/PID复用的 Windows 对抗验收仍保留，不能由 mock 宣称 OS 保证。SDK x64/x86、实际 pipe、host/CLI/GUI 接入与原生验收继续，既有 Windows 原生审批额度拒绝未重试。

依据：[GetNamedPipeServerProcessId](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid)、[process identifiers](https://learn.microsoft.com/en-us/windows/win32/etw/process-v2-typegroup1)、[process termination](https://learn.microsoft.com/en-us/windows/win32/procthread/terminating-a-process)、[GetProcessTimes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocesstimes)。

验证：新增 8 项，覆盖真实 Client machine→production adapter/pipe owner/process pin/token API mock、BUSY 原期限与单次 open、预期 creation/SID/logon/session/实际 pipe PID 错误零 request、连接后 exit/主体/creation/PID 变化、已发 request 后失败不冒称成功、坏回复、pending 保留 pin/多资源独立关闭错误、callback 零 API 和 full FILETIME/存活/query 错误。Linux 全量 1988 通过，8 native IPC 默认 ignored；格式/Clippy/源码 62/现有 Linux/X11 ABI/regex oracle 通过。初始 mock 字段、跨 sibling 测试可见性和两项 lint 已修。新增 Windows SDK/native_link 项未实际运行，真实对抗与原生 gate 不能以 mock 替代。见 [验证记录](validation/linux-2026-10-16-windows-control-client.json)。

后续：host 从 ControlServerInstance.current 取得自身 metadata，按选定实例发布/传递给 CLI/GUI，避免把任意 pipe 对端自报值当可信选择；沿原 Supervisor 接入 Status/Stop，保持公开 session nonce 与私有 worker run 分离。GUI 使用显式 endpoint 选择和 freshness/两次确认，不让用户手输底层 PID/FILETIME 作为正常流程。CLI/GUI 必须区分已提交请求后的不确定结果、实际 worker exit、recovery 与复合 cleanup 事实。
