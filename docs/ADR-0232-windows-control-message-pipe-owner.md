# ADR-0232：Windows 控制消息管道 owner

ADR-0231 的事务需要真正的 message-mode overlapped I/O，不能借用私有 byte-pipe owner 的名称、权限、读流和握手。新增独立 ControlMessagePipe：只接受本地固定 keyweave-control-32hex 名称；server 使用 FIRST_INSTANCE、REJECT_REMOTE、duplex、message/read-message/wait、一个实例和非继承属性，描述符同步短借用 ADR-0230 的具体用户/SYSTEM protected DACL。client 只打开同命名空间，明确 overlapped 和 SECURITY_IDENTIFICATION SQOS，再设 message read mode。身份、设备、抓取与输入不在工厂内隐式启动。

此 owner 是未鉴权的原始 I/O 资源，不实现 ADR-0231 的 ServerTransport/ClientTransport，不调用业务 handler，不能单独暴露为可信公开控制服务。后续必须绑定实际 peer/最后已读消息安全上下文，并恢复线程身份；任意 PID 比较或 DACL 不足以替代。

一个稳定 Box 保存 OVERLAPPED、65-byte buffer 和 immediate DWORD；每次只一个 flight。同步完成从初始调用的 DWORD 获取长度；仅 IO_PENDING 进入后续 GetOverlappedResult(FALSE)。MORE_DATA 无论同步或异步均保留，不把截断当完整消息；read/write 实际长度必须在请求范围内，write 必须完整且 Pending 的输入不能改变。连接完成的 DWORD 未定义，不解释。所有 I/O 均禁止 callback、跨线程和 raw handle 导出。

取消只对本 owner/flight 调用一次 CancelIoEx；取消请求或 ERROR_NOT_FOUND 都不等于完成。后续零超时检查独占 manual-reset event，只有完成信号才可断开、复用或释放内存。close/disconnect 无等待循环、不 FlushFileBuffers；绝对期限由事务/上层负责。API 错误与取消/等待/两个 CloseHandle 错误独立保留；Drop 只一次非阻塞回退，未知 flight 整个 Box/句柄保留至进程退出，线程 sticky poison 禁止继续建管道。callback 内 Drop 不调用 API，未知资源也保留。

工厂只创建/打开自己的句柄，不接管外部/继承句柄；错误明确返回 primary operation、回收快照和独立 descriptor close 错误。后续有限 Drop 不能将该快照重写成成功。

依据：[overlapped pipe server](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-server-using-overlapped-i-o)、[ReadFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile)、[GetOverlappedResult](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresult)、[cancel pending I/O](https://learn.microsoft.com/en-us/windows/win32/fileio/canceling-pending-i-o-operations)。现有私有 byte pipe 不变。新增 native SDK/link 保留项须真实 Windows gate；本环境此前原生审批额度拒绝不重试。

验证：新增 13 项测试：实际生产 owner 的 API mock 路径覆盖完整消息/空消息/MORE_DATA、owner 移动、Pending 不变写、连接未定义 count、损坏 shape/length、取消未完成、factory 与独立回收错误、callback 禁止 I/O 及已关闭 Drop、Rust pointer-width layout，以及实际 Server machine→owner→API mock 的事务（显式 synthetic auth）。Linux workspace 1973 项通过，8 已有 native IPC 默认 ignored；格式/Clippy/源码 62/Linux ABI/X11 SDK/regex oracle 通过。首次 mock 私有字段访问与 collapsible-if lint 已修；没有 Windows 原生/输入执行。见 [验证记录](validation/linux-2026-10-16-windows-message-pipe-owner.json)。

后续接入顺序：

1. 在 owner 内绑定已连接 peer 和最后完整 read 的消息安全上下文。服务端查询 thread impersonation token 的完整 SID/logon/session 并核验，必须在任何返回或 panic 前恢复原线程身份；恢复失败不得继续 host 工作。ADR-0230 primary-token PID 比较不足以完成该授权。
2. 客户端实际连接 server 的身份与存活检查须审查 PID 复用/handle 转交边界；不能只相信命令行 PID、endpoint 字符串或名称存在。
3. 完成实际鉴权后才实现 ADR-0231 两个 transport trait；将冷 open 的 BUSY/NOT_FOUND 重试放入原绝对 1s connect 期限，每 poll 至多一次 open，不 WaitNamedPipe 或自启动服务。
4. host 循环先监督 deadline/exit，再有界 poll 公开 server；CLI/GUI 只复用该公开协议，不把公开请求当进度或续租。fatal/timeout 后按原绝对关闭期限处理 owner.close 快照，不以重新创建 machine 代替取消完成。
5. Windows SDK x64/x86、生产 native_link、真实无输入 message-pipe 正反例全部实际通过后，才能声明 Windows 公开控制可用。
