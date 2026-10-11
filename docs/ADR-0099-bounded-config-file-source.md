# ADR-0099：单任务后台配置文件读取

日期：2026-10-11。状态：Linux 文件候选读取组件已实现；watcher 调度、host 集成与用户 reload 操作尚未连接。

## 冷读取与身份检查

Linux config_source::open 采用标准 OpenOptions READ+O_NONBLOCK（CLOEXEC由std保留），跟随普通 symlink 后检查**实际打开**对象必须 regular。NONBLOCK 防止 FIFO open 等待 writer，不保证普通/网络文件系统 open、metadata、read 有硬时间上界；它没有创建输入/输出设备、grab、ioctl 或协议端点。

host file_source::FileSource 冷构造保存绝对 PathBuf，不访问文件、不丢非 Unicode OS units。request 在唯一 named background thread 中打开文件、检查初始长度1..8MiB、最多读取8MiB+1字节后判断实际上限，保留精确原始正文。UTF8、JSONC/schema、能力、output/Engine 编译仍交给 worker；source snapshot 成功不能被解释为可执行配置。

读取后比较 before/opened-after/current-path 的 dev、ino、长度、mtime/ctime纳秒与mode，要求 current 仍regular且实际正文长度与metadata一致。正常写入/atomic rename 的可见变化拒绝为 Changed，不将已观察到的旧 FD 正文交给新的 pathname。该检查不能证明原子快照、故意恢复 metadata 的 ABA、文件认证或与未来读取一致；保存配置宜用完整临时文件+rename。source 类型不在正文中加入文件名、timestamp、generation或哈希，仍需实际 worker ack。

## owner 与容量

每个 FileSource 最多一个 running job 或一个 Ready 结果，Ready 未 take 也 Busy；不覆盖、不启动另一线程、不隐式重读。poll 仅在 is_finished 后 join 原 handle；运行中的 filesystem job 永不 join/sleep/等待其他线程。take 单次移动 Snapshot/错误，Snapshot.into_bytes 交付 owned正文，Debug 仅长度。

close 永久 Closed、释放已完成结果；未完成 job 保留，后续 reap_closed 只观察同一 handle并丢弃晚到结果，不重启、不激活/传输、不刷新 actor baseline。Drop 可能 detach未完成线程；真实集成必须保留 owner 驱动关闭/回收，不能从文件成功/线程 Drop 推断 child Exit或全局恢复。每 owner 的任务/正文长度有界，线程调度、allocator、已完成 join 与 filesystem IO 不具硬时间上界，也不限制调用方创建任意多个 owner。

SourceError 只保留静态类别及 IO kind/errno，不保存路径、正文或原 error message。内部 panic在join时转为Panicked错误，不产生snapshot；没有替换全局panic hook，因此第三方panic hook日志不由本模块保证。目录/不存在/权限/空文件/超限/改变的候选不会触发actor更新。允许新的显式读取时也仍由同一source owner执行，watcher不得把IO读取成功当确认或为未知commit自动重发。

## 接入顺序

source job必须在 HostSignals 安装与 worker spawn **之后**启动，避免破坏sole-main安装和creating-thread parent-death合约；所有启动/poll/take都在可信host control线程，禁止输入callback/per-key task。parent原NativeSession继续驱动progress/Stop/watchdog，文件工作不造epoch或续租。

下一步添加有界poll/debounce/显式手动读取调度，最多一个source候选、一个parent reload事务与一个未取确认。actor baseline只能在对应实际Accepted后推进；读取错误和Rejected保留已激活配置，Stop要关闭source与lane并沿用原child恢复/Exit监督，不能因读取阻塞停止驱动监督。snapshot交付到NativeSession之前仍需Active/Running准入；变化检测不是配置激活。

## 验证

Linux Rust1.97.1 locked/offline workspace/all-targets938项libtest、既有actual-main no-input reload两场景、Clippy -D warnings、fmt、60项固定来源、现有Linux C ABI oracle通过。新增7项Linux-only tests：exact JSONC/无效UTF8原字节/Ready Busy/单take/Debug隐私，空/8MiB inclusive/超限/目录/缺失及错误隐私，普通symlink和非Unicode，保留旧FD期间atomic替换和写入变化拒绝，自己创建的无writer FIFO不阻塞，停止后保留受控等待线程并回收/丢晚结果，relative拒绝/panic不产正文。

新测试仅自身临时文件/目录/symlink/FIFO和metadata、owned线程；没有查询真实VT/evdev/uinput、全局键盘、挂起、driver或安装。生产代码未增加unsafe/FFI/外部依赖，使用已有libcO_NONBLOCK常量和std文件API。Windows不提供此Linux-only组件，前序native检查仍待补，不能称已完成跨平台文件监听。完整原目标仍未完成。见 [验证记录](validation/linux-2026-10-11-file-source.json)。
