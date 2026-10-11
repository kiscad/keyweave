# ADR-0101：run-linux 文件监听入口

日期：2026-10-11。状态：用户命令和冷准备配对已实现，真实 console 活动捕获/热更新仍待硬件验收；延续 ADR-0100 的后台轮询调度。

## 命令与冷准备

`run-linux --watch-config` 显式启用文件正文变化监听，只有 console provider 可用。`--watch-interval-ms` 要求同时启用监听，范围100～60000，启用后的默认值1000。未启用时沿用原冷启动/监督行为，不保留额外正文副本、不启动文件线程。None 始终永久无设备/无 reload，启用监听在 worker/配置查找前按 Arguments 失败，不为测试添加可激活的 None 模式。SIGINT、SIGTERM、SIGHUP 仍全部请求 Stop，未将 SIGHUP 改为 reload。

`PreparedLaunch::prepare_watched` 返回 `(PreparedLaunch, ConfigWatcher)` 冷配对；原 `prepare/start/StartOutcome` 接口保持。先验证 provider/interval 和原严格设备参数，再定位 worker；读取前将源路径变为 absolute lexical OS path，随后打开该路径、沿用原 regular/有界编译检查。未 canonicalize 成 symlink 目标，因此未来替换用户选定的文件名/symlink 可以被读取。

watcher baseline 的 Arc **直接复制该次 PreparedConfig 的精确正文**，sealed artifact 也来自同一个 PreparedConfig，不为监听基线重开路径或重序列化 JSON。source path 仅保留在 host，worker 仍只收到原 sealed bytes 和原 argv；随机 run 生成/信号准入/child mask/隔离组/父保护/reload 管道流程不变。配对构造不建线程或查询设备；可信库调用方必须将 watcher 与这个 launch 的 Running session 一起使用。

## 活动循环与最终报告

实际 start 返回 Running 后才 poll watcher。每次由 watcher 先驱动原 NativeSession，消费一个状态通知并更新有限、饱和的计数，循环内不读 stdin、不打印日志、不打开/解析配置。原后台任务、progress/Stop优先级和固定 reload deadline继续由 ADR-0094～0100 维护。watcher 错误只保存首个静态类别，并继续驱动同一 NativeSession 的停止/实际退出监督，不重建 worker/source/clock。

最终统计包含提交、Accepted、Rejected、Cancelled、读取错误数量及最近有效 reload 确认代号，不输出正文、源路径、run ID 或输入事件。无 reload 确认时代号为 None，不将启动成功、Submitted 或文件读取成功假作 reload Accepted。正常文件错误/Rejected 保旧配置，用户通过退出后的计数知道结果；**当前没有活动时实时通知或外部手动 reload 命令**，保存文件是此入口的触发方式。

退出或原 ObservationExpired 后关闭 watcher，在独立一次100ms Instant窗口内，按1ms间隔非阻塞 reap 原文件任务。不能取消实际 filesystem IO；未回收报告 `config_source_reaped=Some(false)`，host 返回1。它不删除或替代原 worker Exit/恢复事实，不表示未知文件事务回滚。窗口结束可能仍有 detached线程，不能称成功清理；不在窗口内重启文件任务或刷新任何租约。

实际 Exited 后仍独立审查原 NativeReport.successful；启用监听时另要求无 watcher fatal fault、文件任务已回收。普通 ReadFailed/Rejected 不抹去旧配置的正常运行/恢复事实。观察超时保持 worker_exit=unobserved、返回1并沿用原 emergency Drop；source 回收不能被解释为 child Exit或全局恢复。

## 验证和限制

新增2项冷准备 tests：None/越界在已删除配置及 image 条件下优先 Arguments；自身 symlink/精确正文封存、删除/改变源文件后 artifact仍是原正文、Console argv与cold watcher零job回收。冷测试不调用 start、不访问实际 VT或设备。

新增3项 CLI tests：未启用却指定interval/99/60001三种parse拒绝；None+watch在查找前拒绝；console+watch无效UTF8在启动/信号前 Configuration 拒绝并保诊断隐私。最后一种选择 host 映像，若准入实现错误不能作为正常 fixture；测试必须保持无效配置，绝不换有效 console 配置或真实捕获 worker。

Rust1.97.1 locked/offline workspace/all-targets953项测试、Clippy -D warnings、fmt、60项固定来源校验、host/worker native build通过。既有3场景 actual-main 无输入 reload harness仍通过；其验证后台 source→管道→纯 Engine，未覆盖真实 CLI console捕获启动。实际构建的 host→worker有效配置 **只用 None**，验证 worker78/host1/Missing/recoveryfalse；同有效配置+watch None在父侧 Arguments失败、没有child报告，help包含两flag。没有真实 evdev/uinput/VT/键盘、driver或安装行为。

Windows未执行此轮native补验，新增CLI解析约束为共享代码，平台运行stub仍78，不能用Linux测试证明Windows编译。生产未新增unsafe/FFI/依赖；ABI未变不重跑oracle。日志及最终source/binary校验见 [验证记录](validation/linux-2026-10-11-watch-command.json)。

下一步仍需目录/file rename通知和目标100～300ms去抖、实时用户诊断/独立操作通道、成功 console/硬件验收、Windows reload原生适配/文件源/验证；原开发指导中的高级引擎、图形消费者、热插拔、LED、UI/打包/Rustdriver和companion审计等完整目标不变。
