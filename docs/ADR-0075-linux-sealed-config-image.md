# ADR-0075：Linux 内核封存配置映像

日期：2026-10-10。状态：封存、准入和冷准备接入已实现；Supervisor FD 交付与 worker 入口待连接。

## 问题与实现

Linux worker 不能在父进程冷验证后重新打开用户配置路径，否则 rename/替换/修改可能使执行内容不同于父进程已验证的内容。新增 `keyweave-linux::config_artifact::ConfigArtifact`，使用 memfd 保存精确字节，再设置 WRITE、GROW、SHRINK、SEAL 四种 seal。

创建只接受最多 8MiB 的映像，使用 CLOEXEC 与 ALLOW_SEALING；写入后一次添加全部 seal，再通过统一 admit 检查。任何创建/写入/seal/准入错误都返回原 io 错误或静态诊断，并释放唯一 File owner，不以普通临时文件或较弱 seal 降级。

依据：[Linux memfd_create](https://man7.org/linux/man-pages/man2/memfd_create.2.html) 描述匿名文件、CLOEXEC 和允许 seal 的创建标志；[F_GET_SEALS](https://man7.org/linux/man-pages/man2/F_GET_SEALS.2const.html) 明确 seal 是 inode 属性且不能移除。WRITE 而非 FUTURE_WRITE 用于禁止现存可写共享映射；本构造器不会建立映射，封存失败不返回可交付对象。

## 准入、读取与责任

admit 消费已有 File，先验证读取权限和全部必要 seal，再读取 metadata 大小与 regular-file 类型。它不按名称判别来源，不证明发送者、配置 schema 或 descriptor 的启动来源。未知额外 seal 不削弱四种必要 seal 的要求。

读取使用 read_exact_at，从固定 offset 0 开始；复制或继承 FD 后共享的 seek offset 不影响配置读取。空映像可被封存，但不是合法配置证明；PreparedConfig 的无损编译仍先于 seal。

Debug 仅显示字节长度。as_fd 只提供借用，into_file 交出唯一 File owner；这些是配置资源，不是捕获设备 FD。创建保留 CLOEXEC，不会自行清标志、执行子进程或发送 FD。后续 Supervisor 必须选定交付、worker 必须在原生打开/线程之前验证父保护和继承 descriptor；不能因为持有 sealed file 就绕过这些步骤。

`linux_preparation::PreparedConfig::seal` 使用原保存 bytes，而不是诊断路径或重新序列化配置；已验证的 Arc 快照与 DeviceSpec 不改变。8MiB 上限与 config compiler 的上限由 worker 集成测试共同核验。

## 验证与剩余工作

5 项真实内核、自有资源测试覆盖：写/扩容/截断/追加 seal 被拒绝、CLOEXEC、Debug 无内容、共享 offset 不影响读、alias/原 owner 关闭后仍正确读取、空/精确上限/超限、不封存和 write-only alias 拒绝；另 1 项 worker 冷准备测试证明封存原 bytes。

固定 Rust 1.97.1：Linux 全套 792 tests、all-targets Clippy、fmt、60 来源 hash 通过。扩展并实际运行 C ABI oracle，检查 memfd/fcntl 原型及标志/seal 常量，保留原 evdev/uinput ABI 检查。[验证记录](validation/linux-2026-10-10-sealed-config.json) 记录源码 hash 和日志。

本次只操作自己的匿名内存文件及其 /proc/self/fd alias，不打开键盘、不 grab、不创建 uinput、不加载驱动。Windows 原生验证未执行，新增 artifact 与 seal 方法均 cfg Linux；上一轮 Windows provider 及入口的原生验证仍待补跑，旧报告不能替代。

实际 Supervisor FD 交付、worker claim、consumer-ready provider、完整 Linux bootstrap/host 与硬件验收仍待完成。发现 output evdev 节点不代表桌面开始消费事件，不能用固定 sleep 或自己的 reader 替代该门槛。全部原目标继续保留。
