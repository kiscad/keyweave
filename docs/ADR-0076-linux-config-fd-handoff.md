# ADR-0076：Linux 封存配置 FD 的选定继承与 claim

日期：2026-10-10。状态：Supervisor/启动 token 交付已实现；真实 worker 可执行入口和完整 bootstrap 待连接。

## 交付合约

新增 `Supervisor::spawn_reporting_config`，消费 ADR-0075 的 ConfigArtifact。原两个 spawn 方法保持行为，并移除残留 KEYWEAVE_CONFIG_FD；新模式替换该值为库选定的 FD，不信任 Command 原有环境值。

父进程在自己的权限上下文内以 F_DUPFD_CLOEXEC 复制配置 FD，最低值为 3，避免闭合标准流情况下落入 stdin/stdout/stderr。Command 的 pre_exec closure 持有唯一 File，使该 FD 保持有效直到 spawn 完成。原 artifact 和父侧 closure copy 只按各自生命周期释放，不借用设备 FD。

先注册原有父线程死亡保护，再注册配置交付 hook。后者在子进程 stdio remap 后重查全部必要 seal，读取 descriptor flags，并仅为选定的配置副本清 CLOEXEC。hook 不分配、访问环境、读写配置、修改凭据或等待；错误让 spawn 失败，不返回没有正确交付的 Supervisor。

Command 必须是固定可信 worker，不能在调用方 pre_exec hooks 关闭/重新绑定这个配置 FD，也不能继承设备 FD。本协议不执行全 FD allowlist 扫描，不能声称消除了任意调用方提供的可继承资源。seal 不是 sender/image authentication；不在这里证明调用方已经编译配置。

## Worker 启动

WorkerStartup/ReportingStartup 新增单次 `claim_config`。调用者必须是真实 main、在普通文件/设备/线程之前完成原父保护和协议通道核验。claim 再核验当前父死亡保护，解析非零、无前导零、无符号/空白且可表示为 i32 的 FD，拒绝标准流范围。

claim 是一次尝试，包括失败；重复调用返回 AlreadyExists，不通过重试环境值取得另一份配置。复制输入 FD 时由 fcntl 成功结果建立新 owner，不对任意整数构造借用或接管原 FD。副本默认 CLOEXEC，再验证可读权限、完整 seal、类型和尺寸。

返回 Artifact 通过位置读消费原字节。原继承配置 FD 保留到真正进程退出；关闭返回的副本不是原 FD 关闭证明。核验后继续禁止凭据变更或带资源 exec/fork，不能把 startup token 当作对此类后续修改的强制保护。

## 验证与未完成范围

扩展真实 main 的无输入 worker_parent harness：sealed_config 模式经过实际 fork/exec 和父保护/三协议管道核验，claim 后读到包含 NUL 的精确字节，重复 claim 被拒绝，再按原 synthetic 终止协议完成并观察实际退出。两个旧报告模式确认配置环境残留被移除、缺失 claim 失败且仍不能重试；原父死亡/通道/进度检查保持。

新增 2 项测试：严格 FD 编码边界；stdio/invalid FD 拒绝且复制后关闭不接管原 Artifact。C ABI 增加 F_DUPFD_CLOEXEC、F_GETFD、F_SETFD 检查。固定 Rust 1.97.1 的 Linux 全套 794 tests、Clippy/fmt/source60、扩展 C ABI 均通过。初轮边界测试漏导入私有 helper 导致编译失败，补导入后完整重验。[验证记录](validation/linux-2026-10-10-config-handoff.json) 保留此过程。

这里运行的是自有无输入 fixture，不是真实捕获 worker。没有 evdev/uinput、Hook/SendInput 或驱动操作；synthetic 终止事实不能证明真实输出恢复。真实 Linux main 参数/配置消费、consumer-ready、输出创建/抓取事务与 host 仍待连接。Windows 原生验证未执行，此前 provider/入口验证仍待补跑；所有原目标继续保留。
