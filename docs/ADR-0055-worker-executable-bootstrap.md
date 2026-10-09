# ADR-0055：受监督 Windows worker 的真实可执行入口

## 问题与选择

ADR-0054 的 Worker 是库组件；实际进程必须在初始 main 线程完成父保护准入，再加载配置、安装捕获，并在循环终止后真正退出。新增 workspace 包 `keyweave-worker`，包含共享参数/配置/退出分类库和真实二进制入口。它不是用户控制台或服务：父 host 仍是独立后续任务，不把直接执行该文件当成完整运行方案。Linux 捕获 bootstrap 尚未接入，合法参数在 Linux 明确退出 78。

不用 clap/Result-main：固定私有 argv 不产生 help/error 文本，错误用小整数退出码返回，不向承载进度的 stdin/stderr 调用同步 stdio。初始 main 只设置无输出的普通 Rust panic hook、读取/验证 OS argv，随后立即 WorkerStartup.claim；在 claim 前无文件读取、线程、backend 构造或输入 API。panic 仍失败退出，不证明已完成恢复；运行库的 fatal abort/内存耗尽等不保证能执行 hook，父监督器仍须把破损帧/异常退出当作故障。

## 启动契约

argv（不含 executable）严格为九项：

```text
--keyweave-worker PARENT_PID RUN_ID --config ABSOLUTE_PATH --tag MARKER --identity POLICY
```

父 Supervisor 已提供前三项，extra args 必须为其后六项。PID 是正 u32，run 是正 u64，marker 是正 u32，在 x64/x86 相同；数字只接受无符号、无空白、无前导零的十进制，identity 明确为 strict 或 canonical-scan。后者仅表示 Scan 1 规范位置，不证明 HID 原始身份。marker 仅用于 own injected provenance，不是身份认证；未来 host 负责独立选取/避免冲突，不能把此常量 fixture 值作为产品方案。

解析最多消费十项，缺项、多项、重复/未知 switch、相对路径均拒绝。配置路径保留 OsString，不做有损 UTF-8 转换；绝对路径按目标 OS 判断，不把 Linux mock 路径当成 Windows 合法路径。Windows Supervisor 已对总体 UTF16 命令行做有界预检。argv 本身来自 OS，解析有界不表示命令行内容不敏感；不加入按键/标题或日志。

WorkerStartup 是已有 ADR-0052 的真实 main/Job/父 PID/标准句柄/异步端点核验；失败退出 65，不重试、不降级到直接 CaptureSession。成功后的 owner 保持父 guard 与 private async pipes，在读取配置期间也受父 Job/启动 lease 保护。

## 配置与执行顺序

claim → 打开指定文件 → 最多读取 8 MiB+1 → UTF-8/JSONC 编译 Windows Hook capabilities、generation=1 → executable_without_loss → 构造只分配内存的 SendInputBackend → Worker.install → Worker.run → 检查两种终止报告与 ActorPhase → drop owner → main 返回真实进程退出。

结构错误/非法 UTF-8/过大/IO 故障与 disabled rule 分开保留在 LoadError；不能激活“可执行部分”替代完整配置。shell/JS 不执行，禁用规则及 JSON Pointer diagnostics 在类型化错误中保留。Worker.install 内的 SessionActor 检查实际 native 输出和 Engine 支持先于任何 input probe/notification/Hook；编译通过不代表所有 Windows scan 输出可执行。

字节读取上限不限制文件 IO 耗时；特殊文件、配置编译/读取停滞由独立父进程有限 startup lease 收尾，此时尚未安装输入。没有逐行后台读取/后台心跳。配置由当前用户进程读取，不提供 root/SYSTEM 执行配置动作的路径。当前路径加载是一次性快照，父 host 对配置准备/不可变交付、控制/热更新、诊断通道仍须实现，不能把 worker 本地重读描述成事务化跨进程快照。

## 退出分类与限制

| Code | 含义 |
| --- | --- |
| 0 | 循环为正常终止，已知 shutdown 各结果成功且 ActorPhase=Stopped |
| 64 | 私有 argv 不合约 |
| 65 | 父/Job/管道准入失败 |
| 66 | 配置打开、读取、解析或无损编译失败 |
| 67 | Worker/Session 安装失败（原错误及回滚由库保留） |
| 68 | 执行/进度故障 |
| 69 | 缺 shutdown、Unresolved、unhook/recovery/notification 错误或访问/会话/未知输入丢失 |
| 78 | 尚未接入的平台 |

69 优先于 68，防止主错误掩盖独立 cleanup 故障。FrameSent 不参与成功判断：它可能是旧 Active 帧；终止不等待最终 flush/不增加 epoch。Quit/Emergency/Requested 只有在所有已知 shutdown 结果通过时才归 0。0 表示本进程报告的已知状态，不证明其他来源的全局按键、应用接收或 Hook 可靠性。详细 WorkerReport/ShutdownReport/Unresolved ledger 在 owner 中保持到退出；本轮仅传退出分类，独立有界诊断/控制协议尚待实现，禁止把文本混入进度管道。

安装失败后不重装；owner 有限 Drop 不证明所有清理成功。执行异常尽力 request_stop；普通循环终止已由 Worker 显式 shutdown，Drop 不注入。真正退出后才释放原借用 binary std handles，父进程须用 signaled owned process 观察 Exited，退出码不替代该观察。

## 验证范围

portable tests 检查最大数值/两 identity、非法数值/参数、无限 producer 的消费边界、无损编译拒绝、非法 UTF-8/结构、字节上限/IO 错误、独立 cleanup 退出分类，以及 Unix 非 Unicode 路径。真实 binary tests 检查错误 argv 和直接调用均无 stdout/stderr；Windows 增加真实 Supervisor→owned Job→WorkerStartup→不存在配置路径→退出 66，要求观察真实 Exited、没有伪进度并明确清理两个父 pipe。

不存在的独占命名目录不创建，因此该 Windows test 不能误载一个有效配置后捕获输入。它证明真实入口准入及配置失败顺序，完全不安装 Hook/SendInput/driver，不是正常 capture 端到端测试。退出时 std pipe 可先于 process object signaled 关闭；父 Supervisor 可能独立记录 EOF/BrokenPipe。验收要求真实 code=66，允许保留这条通道故障，禁止为得到 reason=None 而清除证据；lease/其他错误不能冒充配置退出。完整 parent host、用户 stop/config 控制、正常捕获/Hook 活性、外部输入共存、Windows 11/真机/安装与原先 D05/Linux/GUI/driver 目标保留。
