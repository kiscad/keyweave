# KeyWeave Coding Agent 指南

## 当前项目与任务入口

KeyWeave 是 Rust 键盘映射项目，目标 Linux 与 Windows，参考旁边的 `../Karabiner-Elements/`。已创建 Rust workspace 与基础离线切片，真实进展见 STATUS；不要把规划当作代码已完成。

开始前读取：

- `docs/STATUS.md`：真实实现与验证状态。
- `docs/DEVELOPMENT_GUIDE.zh-CN.md`：架构、语义、平台边界与验收。
- `docs/IMPLEMENTATION_BACKLOG.zh-CN.md`：D00–D11/M01 的依赖与工作单。
- `docs/SOURCE_AUDIT.zh-CN.md` 与 `docs/source-manifest.json`：固定来源和证据。

文档与评审说明默认中文，代码标识符、CLI 名、错误 code 用英文。完成当前授权任务并留下可审查的代码与验证证据；普通读写、代码/文档修改、离线测试不另求确认。系统安装、真实全局抓取与加载驱动不由“编写源码”的授权自动包含，按当前用户明确授权与运行环境权限处理。

## 开发顺序与源码边界

- 按 STATUS 继续下一未完成任务；先 types/config/engine/CLI 离线切片，再高级规则、Linux 后端、Windows Hook、会话/GUI。Windows driver 是单独 gate。
- 不修改、格式化或更新 `../Karabiner-Elements/`，除非任务明确要求。不得重写上游 expected fixture 使新实现看似兼容。
- 分析基线 `ec2fea8940f7254a748c052a890ee9db7113d9b1`、version `16.3.11`。source hash 不符时先核验来源变化并更新证据。
- 本地 DriverKit/cpm 子模块未初始化；本次未完成 companion driver 内部审计。不能推断其完整实现、协议或许可证。
- 不因为支持 Rust OS API 就宣称支持所有设备/桌面/游戏。Windows Hook 无可信每事件设备 ID；Wayland 上下文按 provider 探测。
- 不自行引入 C++ 主引擎或将全 Rust driver 目标悄悄改为 C++。Rust driver 原型失败可记录备选方案，保持用户态版进展。

## 模块与代码约束

- TerminalActor 只执行 Output 阶段，arena 的 up 释放保存 Bound；新批次来自当前 Arc，已接收批次保留旧快照。input_up 先 drain due，Reset/remove 控制优先。512 planned/emitted keys 预算包括已完成批次 latent modifier；普通 Engine/CLI 的 duration gate 仍保留。
- action_plan 是已过滤批次的纯计划器，BatchId/slot 由 actor 分配，down/up 复用保存 binding；halt 仍关联原 rule binding，不以批次替代。当前递归 Engine 拒绝非零 hold_down_ns，配置 duration gate 只有在 actor 接入及回放验收后才能解除。
- StageTimeline 只负责有界延后工作，取消会返回未执行 payload；actor 区分尚未执行预算和已执行 owner，不可为取消的 down 编造 up。同刻外部控制/父子阶段优先级仍需明确；现有组件不代表 hold duration 已接入。
- engine/types 必须脱离 OS、GUI、文件系统、系统实时时钟、IPC；所有时间显式注入，可完全确定性回放。
- config 读取 AST、保留元数据、编译严格 IR；OS capabilities 参与编译。runtime 接收不可变快照。
- 后端、session、协议、GUI 有清晰依赖方向；按真实实现再拆 crate，不创建大量没有行为的空包。
- 引擎状态由一个执行者持有，禁止 per-key task、散落的全局可变数据和热路径 `Arc<Mutex<...>>` 组合。
- 固定工具链/MSRV、Cargo.lock；平台依赖放 target-specific 配置。Windows driver 用独立 workspace/WDK，不能拖累 Linux 默认 CI。
- 使用窄类型表达 KeyId、DeviceInstanceId/generation、SourceToken、ConfigGeneration、MonoTime、DeadlineToken。
- `unsafe` 仅在窄 FFI/ABI 边界；每处写 `SAFETY` 解释生命周期、长度、线程/IRQL/对齐依据，不用“OS 保证”泛称。
- 回调不解析配置、不编译正则、不访问文件、不等待 IPC/其他线程、不运行用户命令；输入、输出、deadline 和协议队列都有界。
- 普通控制逻辑返回 Result/诊断；FFI callback 不允许 unwind；生产热路径不使用未证明安全的 unwrap/expect。

## 规则语义不可随意简化

1. HID usage page/usage 为标准物理 KeyId，Unicode/布局/IME 单独处理。左右修饰、numpad 与普通 Enter、Backspace/Delete 区分。
2. 阶段顺序固定：设备修正→simple→complex→平台功能键策略→输出。保持规则数组顺序，设备 simple 优先全局。
3. 按下确定映射和 generation，松开完成原 binding；热更新 invalidates 新匹配，不丢弃 active。
4. physical Repeat 不创建新 owner；未知 up 不释放其他来源。新连接 generation 不接旧 timer。
5. 输出使用 owner set/引用计数，temporary modifier suppression、physical modifiers、locks、sticky/lazy 不混用。普通键多来源改良需独立 golden/ADR。
6. 显式 lazy 只延迟标准 modifier 的输出；halt 的作用域是原 rule binding，当前数组与已安排的 up 必须完成，后续批次共享 halt 状态。
7. mandatory/optional/any 按原定义，缺省 optional 不能等于 any；临时释放 mandatory 后按正确顺序恢复。
8. to_if_alone 是上游语义而非泛用 tap-hold：严格 `< timeout`；其他 key_down/button/wheel 可取消，纯移动在该语义下不取消。
9. held_down/delayed_action/simultaneous 使用可撤销 deadline 与明确边界/顺序。held 的原 up 撤销任务；delayed 的原 up 保留 pending binding，需直到 invoked/canceled/设备撤销完成才释放预算。不要用 wall-clock sleep 掩盖事件顺序错误。运行时先交付到期 TimerFired，再给普通物理事件分配统一 seq；CancellationPending 需分批 CancelDelayed 后重试。Reset 直接检查 deadline/pending 队列为空，不只检查旧回调无输出。
10. 组合键超时回放包含原 down/up 并保持顺序；down/up order、any/all、跨设备与打断有测试。
11. 条件 Unknown 不使 unless 成真；不支持的必要 condition/action 禁用整条规则并提供 JSON Pointer 诊断。
12. eval_js、expression、to_if_other_key_pressed 等延期字段必须识别并诊断。导入/验证绝不运行脚本或 shell。
13. own virtual / injected provenance 在 backend 分类，不靠近时间戳键码去重猜来源，不把输出再次送捕获端。

## 平台和故障恢复

- OutputPump 独占 backend/queue；缺少 REPORT 即使 accepted==count 也 acknowledge_incomplete，先 flush 后恢复 ups。UnknownAcceptance/非法 receipt 冻结 flight，drop 唯一设备后才 backend_destroyed；Quarantined submitted 仅为最后已知 prefix。Detached/Recovery 不自动 resume，真实 capture stop/ready/neutral 由 runtime 落实。
- submitted 恢复批次先释放所有非 modifier（含 Consumer page），再释放标准 modifier；不能按原始 HID 全局排序让消费键晚于 Ctrl up。短恢复只留下未接受的 up，保持原组顺序。
- keyweave-output 跟踪 desired 与已接受的 submitted；短返回只确认已接受的语义边沿前缀，在途未知接受量不可 reset。恢复完成 Paused，runtime 显式通过物理 neutral gate 后才 resume。
- 输出已准备好才抓物理输入，启用/重连等待 neutral。抓取前保留可恢复路径。
- device remove 只释放其 owners，撤销所属 timers/宏；output failure、queue overflow、引擎停滞都进入显式恢复。
- 恢复优先保证物理键盘可输入，不能无限等待失效输出端发 up；清虚拟按住、旧 token，再等 neutral 恢复。
- 紧急物理组合在映射前识别，规则不能覆盖；watchdog 检查事件循环实际进度而非后台线程存活。
- keyweave-progress拥有共享v1 24/40-byte codec/ProgressPhase/watchdog与generic非阻塞Read/Write client；Linux RuntimePhase只是别名，Linux adapter保留Runtime参数与borrowed FD ppoll interests，禁止复制协议实现。Windows progress::ProgressClient只读CaptureSession快照，constructor/receive/publish/flush拒绝keyboard callback。progress_epoch只在实际poll完成（含错误后的shutdown）增加，key/tick/config/手动shutdown不增加；Standby不publish，Unresolved→Stopped只表示循环停止，不表示输出恢复/进程退出。partial reply固定原快照、单flight、耗尽不wrap；已有ADR-0049的原子owned Job启动和无输入父死亡验收，已有ADR-0050的native overlapped pipe pair，但没有child端交付/进度监督/worker启动核验/完整host，见ADR-0047/0049/0050。
- 进度监督采用单在途 challenge：receive 保存 Runtime epoch baseline，后续实际 poll 才 publish，不能用冻结计数/缓存旧请求/后台心跳续约。24/40-byte 固定帧无按键信息，partial/WouldBlock 有界。Supervisor 独占自己 spawn 的 Child，两条非阻塞匿名管道，不按任意 PID kill、不继承设备 FD；signal 不等于退出，必须 poll 至 try_wait Exited。Drop 有限 fallback 不证明已退出；Supervisor pre_exec 已安装 parent-death SIGKILL，创建线程必须保持存活；worker 主线程打开设备前核验 WorkerParentGuard，不得靠 exec 后重装掩盖保护缺失。保护核验后禁止 credential 改变/exec/fork 并继承设备；生产 worker 可执行入口与启动事务仍必做，见 ADR-0027。
- EmergencyChord 按同一物理 device 检测 Ctrl+Alt+Shift+F12，左右均可、连续 >=1s；raw 完整帧校验后先于映射，repeat/重复 down 不刷新、释放撤销、跨键盘不拼接。startup 至少一个可达接口，next_wakeup 含期限，退出优先于普通输入/engine timer/output，不能由规则覆盖。交付时钟积压限制与卡死线程无检测能力需如实说明，supervisor 仍独立必做。
- Linux Runtime 独占输入/Engine/OutputPump；全部 native frame 先验证，不向 engine 交付半帧；SYN_DROPPED 终止 capture，逐设备 normalizer 不猜 MSC HID。到期 TimerFired 优先，CancellationPending 分批取消、保留原 logical key 和 seq。每 poll 64 操作，先处理已排队 due 输出再读新批次；失败/pause/Drop 必须 close 全部 capture 再收尾 output，8 次恢复上限。progress_epoch 只反映完成 poll，不能代替 supervisor；startup 重验 arm 关联的 backend 身份，hot config 先检查全部输出。
- CaptureDevice arm 使用短期 OutputNodeReady，token 仅证明 owned uinput 的 event node 身份/公告能力，不证明 compositor ready。单设备 pre/post neutral、own output exclusion、passthrough coverage 后才 Captured；失败 rollback，ungrab/read 故障 close 唯一源。EVIOCGKEY flush 该 client EV_KEY，限定启用/恢复边界，正常事件循环不得周期性 query；arming 窗口丢 tap 的限制不能被描述成无损原子切换。CaptureGroup 已接入多设备准入协调，consumer-ready 与可执行 bootstrap 仍需落实。
- Linux uinput 正短写必须先补 REPORT 再恢复 submitted ups；errno/非整记录是 UnknownAcceptance，quarantine 后释放物理 capture 并销毁设备，不确认零接受量或重试整批。OutputState::Ready 不等于 capture ready；Recovery 本版不能 resume，需销毁重建。自身识别比较 canonical input syspath，repeat 仅 Kernel 或 External，LED read 不等于同步已实现。
- NodeDiscovery 仅扫描 owned uinput 的 canonical input syspath，最多 64 子项、33 bytes dev 属性，核对唯一 eventN、实际 FD major/minor 和完整身份/能力。不按 eventN 推算 minor，不用名称匹配；缺节点可等，权限/歧义/身份/格式错误失败。默认 2s/20ms、最大 5s/最小重试 1ms，生产 poll_uinput 在探测前后取 CLOCK_MONOTONIC，迟到 Ready 必须丢弃。发现器不内置 sleep，next_wakeup 接既有等待；节点出现/自身 open/udev settle 不证明 consumer-ready，见 ADR-0031。
- CaptureGroup 先全组 inventory/config/output-capability/emergency/repeat preflight，再全部 neutral、逐设备 arm、最后全部 neutral。等待按住时撤销全部 prefix，任何故障保存主错误与所有 rollback 错误、关闭全部源；rollback 失败不能重试。Armed 只能消费移交 Runtime，禁止再次 query；输出 owner 必须持续存活到移交，into_runtime 失败关闭输入先于 backend。全组 neutral 仍是逐个采样，EVIOCGKEY 的 arming 丢 tap 窗口保留，不能声称原子/无损；真实 consumer ready 是独立前提，见 ADR-0030。
- WorkerStartup 在真实 main、设备打开之前验证父保护、正 u64 run ID 与两条不同非阻塞 FIFO 的 read/write 方向，之后只接收一次。仅允许进度管道的 CLOEXEC copies，不复制设备 FD；禁止 File::from_raw_fd 接管标准流。stdin/stderr 生命周期内保留给二进制协议，日志不能写 stderr；client Drop 不证明原标准端点已关闭，只有实际进程退出为准。startup AtomicBool 只管理单次端点 claim，失败后退出、不自动重试。此结构校验不等于跨用户鉴权或输出 ready，见 ADR-0029。
- run_worker 只接收已经准入的 Runtime 和独占非阻塞进度端点；先 receive challenge、实际 poll、publish、再 ppoll。CLOCK_MONOTONIC 绝对纳秒，最近 deadline 与 20ms housekeeping 取最小；EINTR 交还外层，HUP/ERR/NVAL 保留，故障先关闭捕获再有限恢复。writer 仅 response pending 时监听可写，设备 FD 仅 crate 内借用同步等待；不导出/复制。此函数不替代 guard-before-open、多设备 neutral、consumer ready 或可执行 worker bootstrap，见 ADR-0028。
- Linux 唯一 device/file-flag/process-lifecycle/clock/wait unsafe 位于私有 ioctl.rs；crate deny、该模块 allow，其他 workspace forbid。owned evdev FD 不导出/复制；查询错误不是 neutral，keyboard_issues 不是输出 ready/own-device/neutral/grab gate。更改 native ABI 时运行 tools/verify_linux_abi.py（不接触设备）。
- Linux keys 的 CanonicalEvdev evidence 不证明原始 HID usage；不把任意 MSC/PS2 scan 解释为 HID。输出别名/未支持 code 在准入时诊断，normalizer up 使用 saved native-code identity；每设备代际独立，不以 reset 编造 up。键表变更运行 tools/generate_linux_keys.py --check，Linux 可附 --header。
- Linux raw FrameDecoder 是唯一 SYN_DROPPED 恢复层；忽略至 report 后仍需显式内核状态协调/neutral gate，不能再叠加 evdev sync。完整 key prefix 缺少 SYN_REPORT 仍是 incomplete frame，不能仅凭 ack 数判成功。native ABI 仅开放已核验布局，其他目标明确拒绝。
- Linux 不抓自身 uinput，不对所有 `/dev/input` 设 666，不默认抓未支持混合触控接口。SYN_DROPPED 恢复只由选定 raw/sync 层负责。
- Linux 的 repeat 来源只选一个，LED 锁状态来源明确；FD 副本不能阻止 broker/watchdog 释放 grab。
- native/hook_thread.rs 的ACCESS_LOST独立于首HookFault/UNTRACKED/通知窗口锁存：InputAccess、AccessLease、SessionChanged停止标记，monitored_wait对probe Err先标记再保留callback主故障。GuardedBackend含disabled/recovery均读锁存，原receipt仍确认；shutdown先unhook再以access loss路径停止，nonempty/unknown保留submitted为Unresolved，known empty可Stopped。ShutdownReport.access_lost与session_changed/untracked/cleanup分别保存；后续只读成功不清锁存、不自动恢复/重启。无失效证据的ordinary emergency/clock/output保持原有限恢复；不是跨桌面原子性，见ADR-0046。
- WTS通知归native/session_notifications.rs，同线程owned隐藏窗口/class与NOTIFY_FOR_THIS_SESSION，passive Hook之前注册；本session任何事件/非法id/意外NCDESTROY锁存停止，其他session忽略，unlock不自动resume。callback只有scalar/TLS Cell，不借actor或查询WTS；poll入口与passive最后binding检查前验证锁存，Hook安装不得擦掉窗口已收到的事件。GuardedBackend每次submit含recovery前读锁存，真实旧receipt照常确认；非空/unknown ledger停止为Unresolved且不盲发up，空ledger可Stopped。ShutdownReport保留session_changed和NotificationCleanup，注销→销毁→注销class；失败毒化线程，保留primary/rollback。独立SessionNotifications/check_notifications诊断不安装Hook或注入，注册smoke不代替真实状态转移，见ADR-0045。
- WTS访问检查位于native/session_info.rs：本地caller ProcessIdToSessionId明确session，不用console id替代；WTSSessionInfoEx的owned buffer必须WTSFreeMemory，检查完整SDK长度/level1与Active+Unlock，只读取scalar字段，不读字符串/padding。Context pre/post包含SessionIdentity(id,logon)，CaptureSession保留startup baseline，passive末次binding检查之前与active refresh都必须比较baseline；callback禁止WTS/RPC，query失败/未知/锁定/断连/改变不续lease。metadata-only check_input_access公开诊断仍不是lease/neutral token；Windows7反转flag不支持，WTS通知已由ADR-0045接入，独立监督与真实RDP/锁屏仍待验收，见ADR-0044。
- CaptureSession active 启用无按键信息的native GUARD_TAG/UNTRACKED sticky保护：other injected在mutable borrow前停捕获，unsupported Pass、故障放行的down/repeat/非法事件及停捕获到close之间的非own输入标记风险；OwnInjected保持bypass。风险与首个HookFault独立，不能由emergency/lease/clock错误遮蔽。GuardedBackend每批调用前拒绝active fault后的新提交，原API receipt照常确认；unhook后若submitted非空或unknown则quarantine/Unresolved，不发盲目ups；已知空ledger可取消计划Stopped。ShutdownReport.untracked_input必须报告；不是已完成外部共存/逐键碰撞，原close/recovery窗口仍有竞态，见ADR-0043。
- Windows physical Repeat 只读 Engine::physical_repeat_target(STREAM, source)，Rule.repeat_child 指向保留主动作而非modifier scopes；不能取全局最后输出或扫描任意child。要求该source owner仍可见/emitted且desired/submitted都down，再经既有queue/pump提交一条Repeat，共用MAX_WORK；不新建owner、不重播宏、不恢复首次修饰scope、不加软件repeat timer。短/unknown照常停捕获并有限恢复；Linux Kernel repeat与普通Engine/CLI输出不变，见ADR-0042。
- Windows keys 的 CanonicalScan 仅为 scan 位置证据；VK 不替代物理位置，E0 显式区分。132 项表含明确别名/特殊 gate，PrintScreen/Pause/NumLock/LANG1/2 不按普通单字节冒充支持。normalizer 先过滤 injected、保留 down 身份与 Repeat/up；marker 不是鉴权凭据，reset 不造 up。256-edge packet 整批验证不代表 SendInput 接受；native ABI、Hook 与 ledger 单独接入，见 ADR-0032。
- HookDispatcher 首 down 同步保存 Capture/Pass，repeat/up 返回不能改变旧决策；unsupported 整个按住周期 Pass，孤立 up/injected 不触发 handler。错误/panic 保留 evidence、停止捕获；不把当前 fail-open 当作已清 submitted。HookSession 在 native/hook_thread.rs 归同一 native unsafe 边界；callback 先 injected bypass 再 try_borrow，物理重入禁用，GetMessage -1/0 分开，退出/卸载前禁用。close 返回 handler 供有限 output recovery，失败卸载/回调/消息故障禁止线程重注册；模拟 Win32 tests 不安装真实 Hook，生产 Windows 编译/SDK/真机分别证明，见 ADR-0034。
- SessionActor 用一个合并 STREAM，不伪造设备 ID；配置预检所有 branches/modifiers、拒绝 Device condition。标准 scan 全部 Capture+重发，禁止物理 Pass 后重复 passthrough。64 次 key/tick 共用 work，due output→TimerFired→key，CancellationPending 保留当前键重试；ActorHandler 同线程 Rc/try_borrow，clock failure 停止且保留主错误。unhook 后 stop_after_unhook 以最后可信时间 reset、8 次 accepted-ledger recovery；Unresolved 不清 submitted/不重启。物理 Repeat 不重播宏，Windows 按来源保留末端的物理重复已接入（ADR-0042）；原生 timer/neutral/碰撞/监督仍必做，见 ADR-0035。
- SessionClock clone 必须共享 handler/control epoch；SessionControl fresh tick 后取 deadline。WaitPlan ceil 到毫秒、0..20ms；poll_with_control 每轮先 control、64 posted messages 最多、满批不等，空闲再 control 后 MsgWait 零 handles/QS_ALLINPUT/MWMO_INPUTAVAILABLE。Peek/Dispatch/Wait 不跨 actor borrow，control/等待异常先禁用、Quit 禁用，unhook 后外层有限恢复仍必做。20ms 参数不是墙钟时延或 supervision；消息等待 tests 替换 API 不证明 Windows 真机，见 ADR-0036。
- sample_input_state 仅 startup/recovery 且 callback 外，GetAsync 零可能权限失败，不签 neutral token。先 WinSta0 visible/thread UOI_IO/input desktop HOOKCONTROL，再无 impersonation/foreground integrity 校验，high bit 的 254 VK aggregate 后 pre/post 重验。64 WCHAR/256-byte aligned token buffer，SID pointer 仅验证落在返回 prefix 后按字节读取；所有 owned handles 匹配 close、borrowed/pseudo 不关，成功路径 close 失败拒绝。Held/IdleObserved 仍需 passive Hook/安装前后/监督/会话 gate，禁止正常循环全 VK 扫描，禁止自动提权或切桌面，见 ADR-0037。
- CaptureSession 同线程独占 Hook/actor/共享 epoch，配置预检与初次采样先于 passive 注册；2s startup poll 保留所有 Pass binding（含 unsupported），采样前后全部 binding 为空、IdleObserved、无 fault、期限未到才在无 OS 调用的步骤中同步启用。Active 不全 VK 扫描；poll 错误/Quit 先 close Hook 再至多 8 次 actor 恢复，主错误与 unhook/recovery 独立保存，shutdown 幂等，Drop 仅停 Hook 不注入。逐 VK 采样非原子、完整 tap 放行；repeat真机验收/碰撞共存/独立监督/可执行 host 仍必做；紧急组合接入见 ADR-0039。
- Windows EmergencyChord 固定 Ctrl+Alt+Shift+F12 左右任一连续 >=1s，检测原 physical HID，injected bypass 在前、passive 不观察。SessionHandler/active_wait 一个 fresh delivery time，emergency advance 优先于同刻 up、output/TimerFired；repeat 不刷新/建立、组释放撤销、左右重叠不重启。wait 含 emergency deadline；触发专用 HookFault 后 native 立即禁用、CaptureSession 返回 Emergency 并 close-first 恢复；detector 错误不得当成功触发，unhook/recovery 结果独立。Windows 是合并流、允许跨键盘组成，不声称同一 device 保证；卡死/Hook 超时移除/安全桌面需独立监督，见 ADR-0039。
- 活动访问租约用 AccessLease 显式 delivery time：100ms metadata refresh/500ms expiry，callback 在同刻 emergency/actor 工作前验证；monitored_wait 不跨 actor/lease borrow 调 API，due emergency 先于 metadata，成功按完成时刻续期，完成晚于 expiry/倒退/失败均终止。check_input_access 仅 context pre/post，不读全 VK；startup 最后 grant 无 OS 调用。采样可能送 callback，结束先检查 native fault 再续期/tick；checked_control 即使 Ok plan 也检查既有 fault，首 callback fault 优先，禁止继续 Peek/Wait。错误 close-first，不能自动重启/提权/切桌面。变化到下次检查间仍有竞态，100/500ms 非硬实时，不证明应用接收、Hook 存活或 WTS connection；独立进度/session 监督仍必做，见 ADR-0040。
- Windows Hook 运行在会话消息线程，有界同步 capture 决策；不能 queue 后先放行，再发送映射。禁止 RawInput 与 Hook 时戳猜设备关联。
- down 的 capture/pass 策略保持到 up；自身注入 marker + flag bypass，其他 injected 默认 bypass。
- Windows OutputPump 的短正常返回进入 StopRequired，停捕获后显式 begin_recovery，再发已知 ups；恢复短返回不重发 down。SendInput 没有独占虚拟设备，close/Drop/卸载 Hook/进程退出不证明全局键已释放；不得调用 backend_destroyed 或清未知 ledger。Unknown/非法 receipt 保持 quarantine，禁止猜零接受或全键 up；native.rs 为唯一 Windows unsafe 边界，x86/x64 INPUT ABI 与 Windows 编译/SDK/真机分别核验，见 ADR-0033。
- Windows FFI/ABI/constants 改动须在 Windows 跑 tools/verify_windows_abi.py --arch x64/x86 或 tools/validate_windows.ps1 的同一 SDK oracle 与对应 Rust target tests；SDK oracle C11 compile-only，不执行 Hook/SendInput。native_link integration test 依赖生产 crate，保留函数地址证明链接而不调用接口。QS_ALLINPUT 目标 Windows 10/11 为 0x1cff，不能复制旧 0x04ff；脚本/CI 未运行须明确标记，不用 Linux/mock tests 代替，入口 docs/WINDOWS_VALIDATION.zh-CN.md。
- 当前主机是 WSL2，可在沙箱外调用宿主 Windows Rust/MSVC（沙箱内 vsock EPERM 已观察）。tools/validate_windows.ps1 将源码/cache/target放独占 Windows-local TEMP，提供 hash/日志；禁止把 UNC cache 锁/活 PID 当通过，结束确认属于本次的进程树后再换目录。现有宿主 stable=1.99.0；另用独占临时 RUSTUP_HOME 安装固定1.97.1，两种目标 x64/i686 的432 tests、fmt/clippy/link、12 replays与 SDK 均已通过。独立安装使用 rustup toolchain install --no-self-update，避免 launcher 自更新；不更改用户默认工具链，命令始终显式 -Toolchain 1.97.1。真实SDK MSG 无非_MAC lPrivate，size48/28；portable Linux mock路径用 host-absolute，不能削弱生产 is_absolute 或屏蔽测试，见 ADR-0041 与 docs/validation/windows-2026-10-09-pinned.json；旧1.99报告保留历史证据。
- SendInput 处理部分成功与 submitted 状态；UIPI、Hook 超时移除、安全桌面、Raw Input 游戏都是明确限制，不当作自动已解决。
- VHF 只解决虚拟输出；可靠物理屏蔽另做过滤方案。Kbfiltr 不是通用 USB/蓝牙安装器。
- driver callback 不等待用户态、不解析 JSON/规则，IRQL/PnP/电源/Verifier 单独检查；无人连接/租约失效必须恢复。
- root/SYSTEM 不执行用户配置命令。shell/应用/输入法动作归当前 session agent，附 session lease、限时/限量。
- 默认不记录按键或窗口标题。Event Viewer 明确开启、有界内存、退出清空；日志不变成 keylogger。

- Windows OwnedJobProcess使用STARTUPINFOEXW/PROC_THREAD_ATTRIBUTE_JOB_LIST在CREATE_SUSPENDED创建时准入private unnamed Job，不回退Create→Assign、不使用breakaway/TerminateProcess。cb/flags、opaque list sizing query预期122、1..4096 bytes、stable heap/alignment与stable Job array生命周期必须保留；init成功才Delete且恰好一次，Job先于属性创建且后于属性清理。Prepared没有child，ResumeAttempted禁止停止回滚失败后的第二次恢复；request_stop不是Exited，signaled poll才确认（259亦可真退出）。所有权不导出/复制、不按任意PID kill，显式操作禁止callback。tools/check_windows_job.ps1只终止自己的fixture父、保留worker观察句柄证明实际退出，无输入；已测恢复后的父死亡不证明全局键释放。native IPC/进度监督/worker启动核验/Hook存活/host仍必做，见ADR-0049/0048。

- Windows PipeReader/PipeWriter独占不可继承端点，无raw handle/name导出或Clone；pair使用128-bit BCrypt random/first-instance/reject-remote/protected owner-SYSTEM DACL。每端一个stable Box OVERLAPPED/UnsafeCell64-byte buffer/event，单flight，IO只启动或GetOverlappedResult(FALSE)，WouldBlock不ack、不重发在途write，重试必须同prefix；Connect完成count未定义必须忽略。只在completion确认后访问/重用OS buffer。close先CancelIoEx再<=20ms event确认，取消成功不是完成；pending保留owner可重试，Drop仍pending则保留memory/handles至process exit并毒化线程禁止新pair，不能free正在由OS写入的内存。callback禁止factory/IO/close，Drop callback零等待。报告cancel/wait/pipe/event错误，constructor仅primary+Drop rollback不证明全回收。check_progress_pipe同进程fixture epochs只证明真实transport/wire/EOF/cancel，不是CaptureSession活性或完整supervisor；Job精确句柄交付、worker启动核验/host后续必做，见ADR-0050。

## 检查和完成证据

- 本机 Windows Rust 入口为 `C:\Users\sam chen\.cargo\bin`，WSL 使用 `/mnt/c/Users/sam chen/.cargo/bin`；含空格路径必须完整引用。Windows 开发优先复用该原生工具链，按 `docs/WINDOWS_VALIDATION.zh-CN.md` 调用 MSVC/SDK 验证，不更改用户默认工具链。只读版本探测、原生编译、mock 测试和真实桌面输入验收分别报告。

文档阶段：验证路径、链接、manifest 校验和、任务依赖与状态一致；没有 workspace 时说明尚不能跑 cargo。

创建 workspace 后按变化范围运行：

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

平台 feature 用独立构建组合，不能无条件 `--all-features` 同时启用互斥捕获后端。driver 用其独立 WDK 检查，不冒充 workspace tests 已覆盖。

新规则/时序/恢复改动优先新增可失败的 replay/golden/性质测试；UI 文案、纯文档等低影响改动无需镜像实现的测试。通过相关检查后，只有新增变化或未解风险才扩大测试。

fixture 来源固定 commit/path/hash，保留原始与转换后数据；supported/pass/skip 分开。兼容偏差必须附理由和 KeyWeave golden，不能覆盖原 oracle。

必须覆盖：热更新持键、同键多来源、拔设备、stale timer、阈值边界、queue full、部分输出、own output loop、Reset。OS backend 加平台真实故障注入。

Linux/Windows 编译、mock/replay、实际设备验收分别报告。Linux 主机不能证明 Windows driver 可加载；WSL 不能证明物理 Windows 输入可拦截；macOS 上游 tests 本次未运行。

结束时更新 STATUS：task ID、实现文件、检查命令与结果、OS/build/硬件（若相关）、未验证限制。不要用“测试通过”概括未执行的系统检查。功能新变化同步 capabilities、CLI/GUI 诊断、兼容文档。

## 决策和交付

改变架构、所有权、时间语义、平台承诺、配置格式、权限边界或 driver 路线，先写/更新简短 ADR（问题、选择、替代、证据、影响）。普通实现选择在本约束内自主完成。

每个任务交付最小完整行为和相关验证，不停在空接口或 TODO。当硬件/外部依赖阻塞系统验收时，继续完成独立代码、离线验证与文档，准确标记实际阻塞。

新项目许可证和名称公开可用性待选定；不要沿用上游名称/图标、给第三方源码错误套根项目许可证，或默认安装未经核验的第三方驱动。
