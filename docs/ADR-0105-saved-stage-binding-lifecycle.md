# ADR-0105：跨阶段保存绑定的生命周期

## 决定与已实现行为

新增共享纯 Rust `stage_bindings::BindingArena<T>`，保存每个 ActionSlot 的预留、Live(T)、Released 状态。T 仅在对应 Down 真正执行后写入；Up 必须取出该 T，不重新选择规则。Released 状态保留至批次结束，防止同一批次内重复 Down 被当作新绑定。不存在、越界、未执行或重复 Up 均返回 Lifecycle。

每个批次保留完整 PlanOrigin：旧 Arc 配置、设备代际、原始来源、阶段及时间。父 ActionSlot 必须已经 Live，子批次只允许 Simple→Complex→Output，要求同一设备、来源及 Arc 实例；输出阶段不能再产生孩子。每个槽记录未退休子批次数，父 Up 和父取消在孩子仍存在时返回 ChildPending。调用方必须先完成或取消孩子，然后释放父绑定。此结构不负责自动生成 after-up、held、alone 或 delayed 分支。

prepare 在容量、父关系和来源校验后返回独占借用的 Reservation。此时仅创建待提交的数据，不改变 arena；丢弃 guard 即放弃准入。commit 持有原独占借用，父关系及容量不能被其他操作改变，因此无需第二次可能失败的准入。边界为 MAX_BINDINGS=4096 个批次、MAX_OUTPUT_OWNERS=16384 个槽。空批次不预留；调用方处理空计划。内存分配器失败和执行耗时不具有硬实时保证。

cancel 返回实际 Live payload，按槽号倒序，未执行的 Reserved 和已经 Released 的槽不产生清理动作。retire 只接受全部 Released 且没有未退休孩子的批次。父子表只能形成至多两层边的阶段 DAG。批次 ID 的跨生命周期唯一性、队列清除和执行顺序仍由调用方保证；arena 本身不授权输入捕获、激活代号或 native 输出。

## 终端 actor 接入

TerminalActor 的私有 BTreeMap<ActionSlot, Bound> 改为该 arena。enqueue 先生成 reservation，随后进行原时间线原子准入，成功才 commit 并推进原 source cursor 和单调 batch ID；时间线失败丢弃 reservation，旧队列及预留不变。空计划不产生幽灵批次。Down 执行前 check_down，保存实际 Bound；Up 取保存绑定，完成后 retire。取消只释放 arena 返回的实际 Bound；重复事件不能重建绑定。

终端 actor 仍只执行 Output stage。父子关系是已测试的共享基础设施，Simple/Complex 延迟执行尚未集成。reset 丢弃 arena 后调用原 Engine.reset 清理其实际 owner；独立 arena 的 Drop 不执行操作系统恢复。真正输出的接受与恢复证据仍由平台 runtime/ledger 提供。

来源分配继续由 actor 管理。arena 校验的是原始物理 SourceToken 的一致性，不要求子批次的 owner source 区间全局递增；未来根输入可能提前保留整个子树的来源区间。不能把这项元数据校验当作来源预算已完成。

## 验证与后续

新增 3 个集成测试覆盖：未提交预留撤销/同 ID 重试、重复 Down/Up 和未执行释放、三阶段父子关系及退休顺序、旧 Arc/设备/source/stage 不一致拒绝、Output 子批次拒绝、容量及越界、只清理实际 Down 并倒序回收。现有时间线溢出准入重试、持续时间、输入 Up、设备移除和旧配置终端 actor 测试也通过。

Linux 工作区 976 个测试用例通过，Clippy -D warnings、fmt 和 60 个上游源码 hash 校验通过。无新增 unsafe、FFI、依赖或 ABI；未重跑 ABI oracle。Windows 原生编译及前序平台补验仍待进行，Linux 结果不代替 Windows 证据。未操作真实输入、设备或驱动。

接下来将 RuleSelection、ActionPlan、StageTimeline、BindingArena 接入完整 Simple/Complex actor，并处理额外分支的生成时条件、父子结束协调、变量旧快照命名空间及来源/清理预算。生产 Engine 的 UnsupportedActionTiming 拒绝仍保留；simultaneous、原生 duration、Windows adapter、硬件与完整原目标尚未完成。
