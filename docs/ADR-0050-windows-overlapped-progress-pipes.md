# ADR-0050：Windows 有界 overlapped 进度管道

2026-10-09 补充：写端以 `GENERIC_WRITE | FILE_READ_ATTRIBUTES` 打开；属性读取权限只用于元数据，不授予读取协议 payload 的能力。配对在任何 Connect/Read/Write 之前分别查询两端的类型、端点标志、最大实例数和 read/wait mode；严格要求 reader 为 server、writer 为 client、byte type、单实例、byte read mode 与 PIPE_WAIT。API 失败保存原 Win32 错误，结构不符返回 InvalidData，已拥有的两个 pipe/event 按既有回滚契约关闭。查询只取 DWORD；buffer size、collection、timeout 与账号字符串输出全部 NULL。

`PIPE_WAIT` 与异步 I/O 可同时使用，非阻塞性质来自本工厂创建时的 `FILE_FLAG_OVERLAPPED` 和既有完成查询。上述元数据不能证明任意继承 handle 的 async 属性，不可直接作为未来 WorkerStartup 的完整证明；精确句柄交付、来源/父保护核验和跨进程监督仍须实现。写端查询所需权限见 [GetNamedPipeInfo](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-getnamedpipeinfo) 与 [GetNamedPipeHandleStateW](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-getnamedpipehandlestatew)。

首轮 native smoke 发现仅按文档列出的 client/server/type 标志比较会拒绝本工厂自己的 pipe。独立私有管道只读探针在 Windows 10 build19045 实际返回 server flags=0x9、client flags=0x8、两端 max instances=1；额外的 0x8 是创建时设置的 PIPE_REJECT_REMOTE_CLIENTS（SDK oracle 已核验该常量）。因此最终严格要求 flags=`0x8 | endpoint_bit`，同时拒绝缺少 reject-remote、message type 或未知 bit；没有以忽略全部未知标志绕过失败。该平台返回值来自宿主实际执行，不是 Learn 枚举已完整列出的承诺。Windows 11 仍须独立验收，结构不符按失败处理。

共享协议需要真正非阻塞的 Read/Write；匿名管道同步 write 会阻塞，Peek 后读取不能解决写端，PIPE_NOWAIT 也不作为 async 实现。采用已连接、单向、byte-mode overlapped named-pipe pair；后续启动事务通过精确 handle list 交付 worker 端，不把命名端点交给任意客户端打开。

配对使用 BCrypt 128-bit 随机名、first-instance、单实例、拒绝远程客户端以及 protected owner/SYSTEM DACL。server/client handle 都先不可继承；无 raw handle/名称导出、Clone 或设备访问。构造时已打开 client，再 ConnectNamedPipe，处理已连接/同步完成/异步连接；构造错误返回主IO错误，已取得的端点由Drop fallback回滚自有句柄；fallback未完成会毒化线程，不提供“构造失败已全回收”的证明。正式worker启动事务仍须保留其primary/rollback/退出结果。随机名不替代 worker 身份验证或完整启动契约。

每端一个稳定 heap OVERLAPPED、manual-reset event、64-byte buffer，一次只能有一个 IO。OS 可写区域放 UnsafeCell，未完成时不能移动/访问/释放；write 另存 immutable shadow 用于重试一致性检查。Read/Write 每次最多启动一个请求或一次 GetOverlappedResult(FALSE)，无 wait/sleep/后台线程。pending 返回 WouldBlock，不把提交当发送完成；成功完成后才交付 read/write byte count，部分 read 缓存可继续取。Connect 完成的 count 按文档未定义，必须忽略并返回内部零值，不以它校验 frame 长度。caller 换掉在途 write prefix 必须拒绝，不能重新提交。empty buffer 遵守 std IO 契约，flush 不等待。

显式 close 先停新操作、CancelIoEx，再有限 event wait 确认完成；取消请求成功不等于完成。无法确认时保留 owner/flight，可重试 close。Drop 最多等待20ms；仍 pending 时宁可保留操作内存和相关句柄至进程退出，并毒化该线程禁止新建 pair，不能释放 OS 仍会写入的内存。完成后关闭 pipe/event，所有错误有独立报告；有限资源 fallback 不宣称已经回收或进程已退出。

所有公开构造/IO/close 拒绝 keyboard callback；Drop 在 callback 内仅零等待 fallback。本组件不捕获键盘、不 SendInput，不能由 native roundtrip 推断输入恢复。本轮交付 transport 和真实无输入协议往返；Job handle inheritance/worker启动核验、独立 progress supervisor 与完整 host 后续接入，原完整开发范围保持。

依据：[ReadFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile)、[GetOverlappedResult](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresult)、[CancelIoEx](https://learn.microsoft.com/en-us/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)、[ConnectNamedPipe](https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-connectnamedpipe)、[Named pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)。
