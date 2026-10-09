# ADR-0057：Windows host 的独占配置文件与唯一只读 reader

父进程编译配置后，worker 重新打开用户原路径不能保证同一内容。先实现不依赖输入的资源层 ConfigArtifact：8MiB 有界原始配置 bytes 写入随机新文件，protected owner/SYSTEM DACL、非继承句柄、仅 FILE_SHARE_READ、CREATE_NEW 与 FILE_FLAG_DELETE_ON_CLOSE；持有唯一 writer owner，sealed 后不再导出写入操作。同步小块 WriteFile，严格正进度/操作数/长度检查，失败不发布。捕获启动之前准备，禁止 keyboard callback；同步磁盘元数据/IO仍可能阻塞，不能在正在监督的主循环中直接重编译/写文件。

核验实际文件句柄类型、inherit flags、GetFinalPathNameByHandleW 后的 DOS local path，以及最邻近实际 volume path 的 DRIVE_FIXED/FILE_PERSISTENT_ACLS，拒绝远程、无 ACL 或未知状态；metadata gate 在写内容之前。SDDL allocation 与 file handle 的独立清理结果均保留；close 只确认资源关闭，不声称所有 reader 已关闭或目录项已删除。创建失败保留主错误、rollback 与可重试 owner；callback 拒绝零变更，Drop 有限 fallback/poison，不当成显式清理证明。

sealed owner 可准备一次 ConfigReader：DuplicateHandle 显式 GENERIC_READ/flags0/不继承，不能用 DUPLICATE_SAME_ACCESS 把 writer/DELETE 能力交给 child；共享文件位置准备时重置至0，之后 owner 无读写能力导出，无第二 reader/Clone/raw handle 公共接口。reader 可用于本地消费；未来跨进程移交必须再次定位0并消费该 owner，禁止 parent 与 child 并行读取共享位置。ConfigReader 使用已构造的 synchronous disk handle、callback 外有界 ReadFile，失败/异常 count 不捏造内容；未来精确 HANDLE_LIST 移交其唯一 owned handle，关闭 parent reader 副本之后再 resume，worker claim 后消费原继承只读文件对象。reader 失败保留原主错误、close 次错误与可重试 unique owner，锁存 reader_attempted/failed，不能为了成功再准备另一 reader。当前这次资源层不代表该移交已经实现。

共享模式保护普通新 write/delete opens，只读数据交付最终依赖同一 inherited file object；path 是诊断/元数据，不能当作完整端到端不可变交付，也不称其防御同权限进程注入/句柄窃取/特权者。父必须持有 writer owner 到 child 真实 Exited；文件在最后 reader 关闭后才删除。支持当前本机固定磁盘/持久 ACL，网络/未知文件系统失败，不回退宽松 tmp 文件。

依据：Windows文件共享与delete-on-close语义见 [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)；规范实际句柄路径/最近volume见 [GetFinalPathNameByHandleW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew)、[GetVolumePathNameW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumepathnamew)，[GetVolumeInformationW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationw)提供 persistent ACL flag。Rust std reader 的默认 share includes read/write/delete，允许读取已有write/DELETE owner，见 [OpenOptionsExt](https://doc.rust-lang.org/stable/std/os/windows/fs/trait.OpenOptionsExt.html)。只读降权与共享文件位置见 [DuplicateHandle](https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-duplicatehandle)，定位见 [SetFilePointerEx](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfilepointerex)。新API/constants须SDK双arch oracle/生产链接与真实本机文件 tests，mock不能证明共享锁、ACL或删除行为；不执行Hook/SendInput。有效DACL的跨用户验证尚未执行，不把设置descriptor/支持ACL flag当成完整安全审计。

完整host、参数/句柄移交、原始bytes无损编译与native输出preflight、run/marker分配、用户停止/控制/诊断/热更新、真实键盘与原Linux/D05/GUI/driver目标继续。

测试覆盖11个mock/ABI/故障场景及3个Windows实际临时文件行为：读允许、write/delete/rename sharing violation32、独占read duplicate、最后reader关闭后实际NotFound、8MiB exact image/更大拒绝；没有真实worker配置句柄继承。失败的security conversion只在成功后取得descriptor ownership，不能对失败未定义输出LocalFree。新ReadFile/WriteFile原型复用既有OVERLAPPED类型，保留Windows Clippy发现的初次声明类型不一致记录，不能改用lint suppression。
