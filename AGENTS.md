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

- keyweave-output 跟踪 desired 与已接受的 submitted；短返回只确认已接受的语义边沿前缀，在途未知接受量不可 reset。恢复完成 Paused，runtime 显式通过物理 neutral gate 后才 resume。
- 输出已准备好才抓物理输入，启用/重连等待 neutral。抓取前保留可恢复路径。
- device remove 只释放其 owners，撤销所属 timers/宏；output failure、queue overflow、引擎停滞都进入显式恢复。
- 恢复优先保证物理键盘可输入，不能无限等待失效输出端发 up；清虚拟按住、旧 token，再等 neutral 恢复。
- 紧急物理组合在映射前识别，规则不能覆盖；watchdog 检查事件循环实际进度而非后台线程存活。
- Linux uinput 正短写必须先补 REPORT 再恢复 submitted ups；errno/非整记录是 UnknownAcceptance，quarantine 后释放物理 capture 并销毁设备，不确认零接受量或重试整批。OutputState::Ready 不等于 capture ready；Recovery 本版不能 resume，需销毁重建。自身识别比较 canonical input syspath，repeat 仅 Kernel 或 External，LED read 不等于同步已实现。
- Linux 唯一 ioctl unsafe 位于私有 ioctl.rs；crate deny、该模块 allow，其他 workspace forbid。owned evdev FD 不导出/复制；查询错误不是 neutral，keyboard_issues 不是输出 ready/own-device/neutral/grab gate。更改 native ABI 时运行 tools/verify_linux_abi.py（不接触设备）。
- Linux keys 的 CanonicalEvdev evidence 不证明原始 HID usage；不把任意 MSC/PS2 scan 解释为 HID。输出别名/未支持 code 在准入时诊断，normalizer up 使用 saved native-code identity；每设备代际独立，不以 reset 编造 up。键表变更运行 tools/generate_linux_keys.py --check，Linux 可附 --header。
- Linux raw FrameDecoder 是唯一 SYN_DROPPED 恢复层；忽略至 report 后仍需显式内核状态协调/neutral gate，不能再叠加 evdev sync。完整 key prefix 缺少 SYN_REPORT 仍是 incomplete frame，不能仅凭 ack 数判成功。native ABI 仅开放已核验布局，其他目标明确拒绝。
- Linux 不抓自身 uinput，不对所有 `/dev/input` 设 666，不默认抓未支持混合触控接口。SYN_DROPPED 恢复只由选定 raw/sync 层负责。
- Linux 的 repeat 来源只选一个，LED 锁状态来源明确；FD 副本不能阻止 broker/watchdog 释放 grab。
- Windows Hook 运行在会话消息线程，有界同步 capture 决策；不能 queue 后先放行，再发送映射。禁止 RawInput 与 Hook 时戳猜设备关联。
- down 的 capture/pass 策略保持到 up；自身注入 marker + flag bypass，其他 injected 默认 bypass。
- SendInput 处理部分成功与 submitted 状态；UIPI、Hook 超时移除、安全桌面、Raw Input 游戏都是明确限制，不当作自动已解决。
- VHF 只解决虚拟输出；可靠物理屏蔽另做过滤方案。Kbfiltr 不是通用 USB/蓝牙安装器。
- driver callback 不等待用户态、不解析 JSON/规则，IRQL/PnP/电源/Verifier 单独检查；无人连接/租约失效必须恢复。
- root/SYSTEM 不执行用户配置命令。shell/应用/输入法动作归当前 session agent，附 session lease、限时/限量。
- 默认不记录按键或窗口标题。Event Viewer 明确开启、有界内存、退出清空；日志不变成 keylogger。

## 检查和完成证据

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
