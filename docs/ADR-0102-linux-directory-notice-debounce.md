# ADR-0102：目录变化通知与有界去抖

日期：2026-10-11。状态：Linux watcher 已接入配置目录通知/200ms去抖与原周期兜底；成功 console 捕获/热更新硬件验收及 Windows 适配仍未完成。

## 原生边界

新增 `keyweave_linux::config_notify::DirectoryNotice`，监视选定文件名的父目录，按原始 OS filename bytes 过滤 MODIFY/ATTRIB/CLOSE_WRITE/CREATE/DELETE/MOVED_FROM/MOVED_TO。原子替换文件仍由目录中的目标名称事件提示，不依赖旧文件 inode，不解析配置、不配对 rename cookie、不把通知当内容认证或实际激活。

仅私有 ioctl.rs 增加 inotify_init1/add_watch 的窄 FFI：创建唯一 NONBLOCK/CLOEXEC OwnedFd，借用 live CString 同步注册 ONLYDIR，任何注册失败沿原 owned FD Drop关闭。公开对象无Clone/Debug/FD导出；使用 std File 的单次 read，最多4096 bytes，WouldBlock/Interrupted不重试。每条16-byte header按native endian逐字段解码，无指针转换/未对齐struct读取；检查有界长度、4-byte alignment、名字NUL与零padding、原watch descriptor，忽略不相关文件。未知/畸形记录以静态InvalidData错误关闭提示通道，不包含目录/文件名。

队列溢出只标记需要重读，不制造具体文件变化或代号。目录自身移动/删除、卸载或IGNORED标记lost，关闭通知对象并记录静态Changed；保留原文件路径周期读取。父目录被换掉、symlink目标在其他目录被修改等情况可能没有可用提示，仍由周期完整正文检查兜底。没有自动重新注册目录或watch descriptor重用路径，不能称所有目录生命周期已事件化。

接口依据：[Linux inotify UAPI](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/inotify.h)、[内核通知队列与read实现](https://raw.githubusercontent.com/torvalds/linux/master/fs/notify/inotify/inotify_user.c)。本地C oracle额外验证struct大小/offset、flags/mask和两个函数签名；不是从网络接口描述推断本机ABI正确。

## 注册与任务生命周期

FileSource::new 保留旧无通知行为；with_notifications 冷构造只设置开关。ConfigWatcher默认选择with_notifications。首次后台文件任务先做目录注册，再执行原raw读取/比较，结果同时携带 snapshot或读取错误与独立通知结果；注册仅一次。没有第二注册线程，没有control循环路径lookup/add_watch，也不在HostSignals安装或worker spawn前建线程。

主线程只在原job is_finished之后join/移动结果；即使文件已删除，目录注册结果仍能交付。如果注册不可用，缓存 SourceError kind/errno，继续原周期读取，不停止旧配置、不启动替代注册线程。source.poll_notice 最多一次非阻塞4096-byte读取；每次watcher advance都处理通知，即使actor事务或通知槽忙，也只保留dirty状态。通知IO与独立progress/reload IO分开，每poll原监督仍先执行，不续任何parent/session租约。

source关闭释放已收到的通知对象。运行中原job可能尚在注册或filesystem IO，关闭不会抢占；保留同一个handle，reap后丢弃晚snapshot和晚owned通知，不启用新watch或交付正文。关闭/reap/Drop不证明child Exit、全局恢复或内核close的硬时间上界。新增代码不承诺文件系统注册、allocator、线程、close/析构具硬时间上界。

## 去抖与确认

首次相关提示记录dirty_since，每次相关提示更新quiet_until=now+200ms；200ms安静后检查，持续变化最多合并到首次提示+1s后允许检查。这个上限只约束调度条件，原source任务/actor事务/未消费最终通知仍可阻止下一次读取，不保证一秒内读取/编译/激活完成。

有dirty状态时周期到期也等待去抖条件，避免100ms轮询绕过去抖；无提示时按原--watch-interval-ms周期兜底。显式request_read可立即请求，仍单标志合并，不扩展原事务期限。开始下一次原任务时清dirty；忙期间的新提示继续积累，确认后再检查，无unbounded queue/逐事件任务。

通知、去抖、读取与Accepted确认继续分离：只有原lane及时有效Accepted+实际observed=base+1推进active baseline；Rejected/Cancelled/IO失败保旧配置；已接受/已拒绝正文抑制重复，恢复已接受正文不增代号。主循环仍不打印通知；终态追加config_notify_error为缓存的通知错误，None表示未观察到错误，**不证明通知完整性或始终可用**。

## 验证

Linux Rust1.97.1 locked/offline workspace/all-targets960项测试、Clippy -D warnings、fmt、固定来源60项、扩展C ABI oracle与native build通过。新增7项tests：2纯记录混合/过滤/overflow/lost/畸形；2自身目录的非阻塞CLOEXEC/无事件/无关文件/nonUnicode/写入/原子替换/删除和父目录删除；2模拟调度200ms边界/持续提示1s上限/忙确认后合并；1后台注册失败只尝试一次而后读取恢复。原受控Stop测试另携带晚到真实owned通知，关闭后同job回收并丢弃结果。

既有actual-main无输入harness的第三场景改用60s周期，每次observe限5s；相关文件变化不调用manual请求，atomic rename→Accepted2、无效覆盖→Rejected2、恢复Accepted正文→Unchanged、删除→ReadFailed，证明这些变化由通知触发而非60s兜底。只有重复坏正文检查保留显式manual；随后仅向自身父PID SIGINT，原worker actual0/Cooperative、原source回收。输入/Active/session仍合成Engine证据，terminal Missing/recoveryfalse/successfalse；不是console消费者/键盘验证。

初次Clippy仅因manual_is_multiple_of和FFI item位于test module之后失败，改标准方法/移动声明后完整重验，未放宽生产lint。所有新增原生动作仅自身临时目录/文件、自己的通知FD及进程metadata；没有实际VT/evdev/uinput/全局键盘、driver或安装。Windows未运行补验，不提供此Linux-only通知后端。见 [验证记录](validation/linux-2026-10-11-directory-notice.json)。

下一步仍需实时通知/独立手动操作、目录丢失后受控重新注册、Windows文件源/reload原生适配/验证、成功console硬件验收，以及高级引擎/图形消费者/hotplug/LED/UI/打包/Rustdriver与companion审计等原完整目标。
