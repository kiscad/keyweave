# ADR-0058：Windows 自有 Job 中的只读配置文件对象交付

ADR-0057 准备 sealed 原始配置 bytes。worker 重新打开路径可能打开另一个文件对象，因此增加显式四句柄启动契约：stdin progress reader、stdout NUL、stderr progress writer、唯一只读 config reader。旧三句柄 API 继续存在；完整 host 必须使用四句柄契约，不以原用户路径重开替代。

## 父侧事务与生命周期

`OwnedJobProcess::spawn_suspended_with_config` 消费 ConfigArtifact 与两个 idle child progress 端点，预检 exe/原 argv/两个固定 suffix 参数/最宽 usize 数值长度，再准备原文件对象唯一 GENERIC_READ duplicate。reader 再次 rewind0 并锁定 transfer 状态，此后本地 Read/第二 transfer 均拒绝。sealed writer 不导出 IO；parent 和 child 不并行读同一共享 cursor。

ChildProgress 仅标记四个实际 owned handles 继承；Job/原 writer/父 progress 端点/event 都不加入 HANDLE_LIST。四项稳定数组与 Job list 同时交给原子 suspended CreateProcessW。原 command 最后增加 `--keyweave-config-handle VALUE`，不返回 raw handle给 caller、不通过环境变量/原文件路径另开资源。删除 attribute list 后对所有四项清继承并分别关闭 parent child-end copies，全部 cleanup 成功才可 resume。调用方仍须避免与其他线程的 ambient inheriting spawn 并行；HANDLE_LIST不能替另一个无列表 spawn 隔离父进程所有继承句柄。

reader prepare/rewind 主错误、独立 rollback/可重试 unique reader、四端 cleanup、Job stop 错误分别保留。已创建但启动失败的 child 永不能通过 cleanup retry 再 resume。`config_preparation_error` 保留历史错误，`close_config_preparation_reader` 显式重试 reader 清理。父 sealed writer 留在 OwnedJobProcess，`close_config` 仅在 Prepared（没有 child）或由 signaled owned process 确认的 Exited 开放；Requested、kill 成功、pipe EOF、worker report 都不能当退出证明。

Drop 有界 stop/Job-close/exit observation，若最后仍未确认退出，刻意保留 sealed writer 的 OS 资源到 parent process exit，不先解除仍可能活着的 child 文件的共享保护。这样可能保留临时文件和句柄；不得描述为显式 cleanup 成功。已确认退出时可正常 Drop，但发布流程仍应显式看 close report。最后 config reader 关闭才真正 delete-on-close；原 writer close report本身不是删除证明。

## child 准入与真实 executable

真实 main 先 `WorkerStartup::claim`（实际父 PID、Job、std bindings、private progress channels），随后同线程 `claim_config_reader`。每个 guard 只有一次 config claim，callback 拒绝不改变 claim 状态；失败需要退出，不能试另一个 handle。拒绝0/MAX/原三 std handles；借用候选 handle 查询 FileAccessInformation，要求精确映射 GENERIC_READ 的 `0x120089`，FileModeInformation 的同步位必须为 NONALERT `0x20`，不能是 async/ALERT。沿用稳定 Native query 输出，在非零 status（包括 pending）保持分配到退出，不读取未完成输出、不关闭未确认的候选 handle。

再检查 disk 类型、继承 flags1、实际 DOS local file/最近 fixed persistent-ACL volume，rewind0，清继承后才将原继承对象接管为唯一 ConfigReader。不 duplicate 第二个 config reader、不把 file/std handle 混用、不暴露 raw/Clone/Seek/Write。此准入检查结构与权限，不是跨用户鉴权、OS 初始线程证明或同权限进程注入防御；同权限/特权攻击不在本契约的安全结论内。

keyweave-worker 参数仍保留旧九项，四句柄模式额外有固定两项 suffix；最多消费12项来判定拒绝，numeric canonical positive/usize 有界、拒绝MAX。原 `--config ABS_PATH` 在四句柄模式只作兼容的诊断路径，不会被打开。claim 后从 ConfigReader 有界加载/无损编译，并显式 close reader，任何 read/compile/close 故障先于 input installation。只有成功编译与清理后才构造 backend/安装；legacy路径读取只在没有显式handle的模式。整数退出码不能保存全部主/次诊断；完整 host 控制/诊断事务仍需补齐。

## 依据与验收

精确继承列表和 bInheritHandles要求见 [UpdateProcThreadAttribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)；继承后的文件对象与权限语义见 [Inheritance](https://learn.microsoft.com/en-us/windows/win32/procthread/inheritance)。原 writer 保护/降权/read共享 cursor 依据沿用 [ADR-0057](ADR-0057-owned-config-artifact.md)。Native 信息类别与同步模式见 [NtQueryInformationFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntqueryinformationfile)。没有新增 FFI 布局/声明；仍验证 SDK与生产链接。更严格 native masks 的适配范围由真实双架构 fixture限定，不能外推所有 Windows/filesystems。

新增行为测试覆盖 transfer 后禁止本地IO、callback/失败锁存、精确四handle/关副本先于resume、实际退出才允许release、独立reader prepare+rollback、partial inheritance/cleanup和历史错误、Drop未确认退出保留writer、suffix preflight/参数边界、child模式/权限/flags拒绝与pending稳定buffer。

无输入 `check_config_handoff` 在真实main准入后读取固定 synthetic bytes（含NUL/非UTF8），验证same-object exact bytes/重复claim拒绝；另验证bad handle拒绝、admitted reader后stall由父owned Job停止，实际观察exit与最后 close 后NotFound。fixture epoch只说明本例程已读完，不证明 CaptureSession 活性。实际 keyweave-worker integration只交付永远无效的UTF8/JSON，确保 Config66在安装之前；fallback路径位于唯一不存在目录，不能因测试误读有效用户配置而抓取输入。无Hook/SendInput/driver/用户配置访问。

完整 Supervisor config factory、parent host/config compile/preflight/run-marker分配/用户控制/完整诊断/热更新与正常 capture尚未完成。原Linux bootstrap/consumer-ready、D05高级规则、GUI/安装/driver/companion审计与既定全部目标继续。此前x86异常仍未解释；新的green checks不能称其修复。
