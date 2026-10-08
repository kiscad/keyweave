# Coding Agent 实施工作单

所有任务初始为未开始。本清单与 [开发指导](DEVELOPMENT_GUIDE.zh-CN.md) 配合使用，状态以 [STATUS](STATUS.md) 为准。执行后写实际证据，不根据设计文本自动把任务标完成。

任务可拆分 PR，但每个 PR 必须交付一个可验证切片。没有真机或权限时，完成不依赖它的代码/回放/编译，并把平台验收保留为未验证。

## D00：固定基线和兼容决策

依赖：无。主要文件：`docs/source-manifest.json`、`docs/adr/`、fixture 导入 manifest。

1. 复核本地 HEAD、status、version、submodule 状态，与 source manifest 校验和比较。
2. 建 ADR-0001：Linux-first、Windows Hook 的限制、driver 单独 gate、macOS 暂不实现。
3. 建 ADR-0002：HID KeyId、输出 owner set、Unknown 条件策略、兼容分级。
4. 选择第一批 fixture，保存源 commit、源相对路径、适用许可证、转换规则、hash；不更新上游 expected。
5. companion 子模块可用后补审计；不可用时明确保留缺口，不阻塞独立 Rust 引擎。

完成条件：设计选择可追踪；所有固定文件 hash 一致或差异有说明；无修改上游源码。验证：元数据和 fixture manifest 校验。

## D01：最小 Rust workspace 与离线切片

依赖：D00。创建 types/config/engine/CLI，其他包在实际需要时创建。

1. 选择可用稳定 Rust、固定 toolchain/MSRV，明确 Edition；锁定依赖并提交 Cargo.lock。
2. 创建可在 Windows/Linux 构建的 workspace，types/engine 不依赖 OS API；新项目许可证决策单独记录。
3. 定义 InputEnvelope/MonoTime/KeyId/DeviceInstanceId、基础 config IR 和诊断结构。
4. 实现合成 a→b、CapsLock→left_control、down/up、Reset；replay 从文件读事件与时间。
5. 加测试：按下后替换配置，未知 up、repeat 不增加 binding、两来源同键输出不提前 up。
6. CLI 输出明确“不访问真实设备”的离线结果与失败退出码。

完成条件：最小 replay 可以预测输出，Reset 后 owners/bindings/timers 为空；Linux/Windows 纯 crate CI 通过。目标命令：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。尚无 workspace 时先创建，不运行假命令。

## D02：统一键码与设备契约

依赖：D01。主要模块：types、config key names，未来 OS translation tables。

1. 按固定上游 momentary_switch 表构建标准键名→HID usage，覆盖左右修饰、numpad、consumer、ISO/JIS。
2. 定义 native key 保存、平台能力与可靠 identity 标志；未知 key 不编造转换。
3. 定义 persistent selector 与 connection generation，序列号可空，复合接口不混淆。
4. 列出 Windows VK/scan/E0/E1→KeyId 与 Linux KEY_*→KeyId 表，表中区分没有等价值。
5. 新 generation 不引用旧 token，unknown native passthrough 与改写输出分开。

完成条件：常用键 alias、左右键、Enter/KeypadEnter、Backspace/Delete 不冲突；不可转换键有诊断；持久选择策略可解释。验证：表驱动映射、round-trip（只对可逆项）、设备重连测试。

## D03：配置 AST、JSONC 与 IR 编译

依赖：D01/D02。主要模块：config、CLI validate/import。

1. 实现 profiles/selected、simple modifications、complex rules、description/metadata、enabled、参数覆盖。
2. AST 保留未识别元数据；IR 严格验证功能字段与 capability，数组顺序保留。
3. 接受合法 JSONC，保留行列/JSON Pointer，字符串中的 URL/注释标记不被删错。
4. native config 扩展仅在 keyweave namespace，编写版本升级/降级拒绝策略。
5. 不支持 action/condition 禁用整条规则并报告；语法/结构错误拒绝整个快照。
6. import 不执行 shell/JS，不激活配置；产出副本与机器可读报告。

完成条件：任何被禁用的条件都不会让规则更宽泛；不能执行的 eval_js/expression 等有显式诊断。验证：上游 core_configuration/simple_modifications/规则 parser 数据、恶意深度/大小、JSONC、metadata 往返与缺少 selected profile。

## D04：完整 C1 映射状态机

依赖：D03。证据：E01/E05–E12/E20。

1. 实现阶段队列、规则顺序与消费，不把 HashMap 顺序当优先级；设备 simple 在全局之前。
2. 实现 mandatory/optional/any、左右与抽象修饰。
3. ActiveBinding 记录原始事件、source、generation、rule、输出动作与 consumed modifiers。
4. 有序输出、to_after_key_up、to 条件；临时修饰释放/恢复和输出 owner set。
5. 热替换快照：旧 binding 完成 up，新 down 用新代；旧 rule 不接新输入。
6. 添加输出故障/Reset 接口，支持 desired/submitted 状态和部分成功。

完成条件：C1 可支持 fixture 全通过；同键多来源、修饰碰撞、跨阶段、profile/application 变化有明确结果。验证：上游 manipulator/event_queue/modifier 测试转换、性质测试。任何有意偏差保存独立 golden 与 ADR。

## D05：定时、组合键与变量（C2）

依赖：D04。证据：E08/E13/E14/E19/E28。

1. 单调虚拟时钟、deadline heap、可撤销 token；不 sleep/per-key spawn。
2. to_if_alone：严格小于时间边界、other down/button/wheel、纯 move 不取消。
3. held_down、delayed_action：按上游 setup/cancel/active 生命周期实现。
4. simultaneous：窗口、strict/inverse down/up、any/all、uninterrupted、超时回放、重叠规则和跨设备。
5. typed variables、set/unset、key_up_value 与变量条件；expression 仍诊断不支持。
6. repeat、lazy、halt、hold duration 的接口和已经支持的子集；其余不得静默接受。

完成条件：所有可支持上游时间 fixture 通过，阈值 ±1 单位与恰好阈值测试通过，设备/配置 Reset 后无旧任务输出；不支持的选项拒绝。验证：纯 replay、proptest 随机交错与边界 corpus。

## D06：Linux 设备透传与虚拟输出

依赖：D02/D04；带高级规则的交付还依赖 D05。主要模块：backend contract/Linux。

1. 前台 explicit device 模式：列举→打开→查询→uinput create→ready→neutral→grab。
2. 非阻塞 evdev batch、native passthrough、own virtual 排除；EV_KEY=2 repeat，不当新 down。
3. 实现 uinput emit/SYN_REPORT/短写，消费者键码映射，不支持 capability 明确报错。
4. raw/sync 模式二选一，确认 SYN_DROPPED 谁负责恢复。
5. 拒绝未覆盖混合接口；读失败/ENODEV/HUP 能 ungrab 与销毁输出。
6. 同键 owner 聚合与输出 capability 更新策略；保证输出未 ready 不抓设备。

完成条件：测试设备透传只有一份输入，a→b/ctrl mapping 在 Wayland/X11 普通应用可用；自设备不会抓取；未抓设备没有虚拟重复。验证：dedicated uinput 集成、真机显式键盘，记录 OS/kernel/桌面/设备。没有设备权限时标集成未执行。

## D07：Linux 热插拔、会话与独立恢复

依赖：D05/D06。主要模块：runtime、broker/session、Linux packaging 原型。

1. 设备 add/remove、复合 parent/interface、持久 selector、generation，重插等待 neutral。
2. 输出 ready 与 capture 状态机；device remove 仅释放所属 owners。
3. 配置目录监听、原子发布、last-known-good、parse/permission 错误反馈。
4. 紧急物理组合与 pause/resume；broker/watchdog 检查实际 engine 进度，FD 不泄漏/不保留阻塞恢复的副本。
5. seat0、当前用户、锁屏/注销/VT 未知暂停；root 不执行用户命令。
6. 定义重复来源和 LED 反馈策略，GNOME/KDE/X11/VT 分别测。

完成条件：kill runtime、引擎停滞、队列满、输出端失败、拔键盘都恢复物理输入并清输出；两设备同键拔一个不释放另一个。验证：故障注入报告、恢复时间、服务停/起和 CPU/延迟。broker 尚未实现独立恢复时只标前台实验版。

## D08：Windows Hook 与 SendInput

依赖：D02/D04/D05。主要模块：backend-windows、会话 runtime、doctor。

1. Win32 message thread 持有 engine，Hook 回调有界同步决策，其他 IO/编译/用户动作移出回调。
2. hook nCode、CallNextHookEx、session desktop、E0/E1/scan key 解码、重复辨识。
3. 注入 source marker + injected flags；自身输出 bypass，其他 injected 默认 bypass。
4. CaptureDecision down/up 固定，simultaneous consume 后 timer replay，不能先放行再追加输出。
5. SendInput 短返回、UIPI/桌面切换、进度停滞、紧急退出、submitted 状态恢复。
6. RawInput 仅设备查看；device conditions 不支持整条禁用；没有 Hook 存活可靠证明就不显示“监控正常”。

完成条件：标准快捷键与高级时间规则正常，无环/双输入；与未捕获物理键碰撞有测试；用户可恢复。验证：Windows 11 x64 编译和真机，普通/管理员程序、中文布局、PrintScreen/Pause、Injected、睡眠/桌面/RDP 记录。仅交叉编译不能标通过。

## D09：协议、上下文、GUI 与 Beta 包

依赖：D07 或 D08 的可用后端；共享 DTO 可在 D04 后先实现。

1. 协议版本/帧上限/request IDs、peer credential/pipe ACL、session lease，拒绝 remote/过期请求。
2. capability、device/status/diagnostics、activate generation、metrics；CLI/UI 同一控制面。
3. 应用 provider：Windows exe、Linux X11 class；Wayland 插件逐环境加，Unknown 不使 unless 成真。
4. GUI：profile、simple、complex editor、导入报告、设备、能力、显式 event viewer、暂停/恢复。
5. session-only program+args 动作；shell 默认关闭；触发不会阻塞映射，不以 root/SYSTEM 执行。
6. Linux 包/unit/udev 与 Windows 会话启动/安装器，配置 backup、升级 rollback、卸载，发布文档。
7. 中文字体、键盘操作、可访问性和托盘关闭行为验收；UI 关闭后映射继续。

完成条件：导入丢失能力可见，激活失败不会显示成功，未授权进程不能控制 broker/读取 Event Viewer；安装升级卸载均不影响正常键盘。验证：协议 fuzz、权限负测试、各包目标真机安装/卸载。

## D10：Windows 全 Rust 驱动研究门槛

依赖：D02、D04 输出/恢复契约、D09 协议规范；不阻塞 Linux/Hook Beta。

1. 固定 WDK、windows-drivers-rs revision 与 toolchain，单独 no_std workspace；记录 FFI/IRQL/生命周期。
2. 先做可安装/卸载、无人连接时正常透传的键盘过滤原型；不直接拷 Kbfiltr INF 到全部设备。
3. 确定 USB/蓝牙/PS2/I2C 的过滤位置，物理 device ID 与虚拟 own device 排除。
4. 实现有界队列与读写 ABI、授权 IOCTL；callback 不等用户态，不解析 JSON、不执行规则。
5. 按处理进度更新租约/watchdog，down 已被吞/客户端死/physical up 后来的恢复基线测试。
6. 分别验证受控 reinjection 与 VHF，选择一种输出方案并写 ADR；鼠标/consumer 单列能力。
7. 测 Verifier/PnP/电源/HVCI/SecureBoot、升级回滚/恢复；产出是否继续全 Rust 的证据。
8. 实际正式签名分发是后续 gate，不以测试签名代替；维护能力/证书/认证与硬件预算列清楚。

完成条件：通过开发指导 §6.3–6.4；两个 USB 键盘可靠区分、蓝牙验证、崩溃/队列满不会永久吞输入、有可操作卸载。未通过标 research 未完成，不标产品支持。若 Rust 工具链不可满足，记录问题/可选桥接评估，先交付 Hook，不自行切成 C++ 驱动。

## D11：扩展能力

依赖：共享生命周期已验收，按功能选择后端。

逐项任务：pointer mouse_basic、mouse motion→scroll/key、高分辨率 wheel、mouse_key、sticky、to_if_other_key_pressed、输入源 provider/selection、expression 子集、JS→JSON 离线生成。每项都有 capability、诊断、golden、故障恢复与平台矩阵。触控板/手柄/Apple vendor keys 不顺带宣称支持。

完成条件：每一项实际可以执行并列出平台范围；未覆盖字段不因 C4 计划存在而被接受激活。

## M01：持续验证与公开声明

依赖：D01 后持续，在每个功能新变化时运行相关检查。

1. CI 报告纯引擎/目标平台编译/真机三栏；fixture 总数、支持数、通过数、skip 原因单列。
2. fuzz 配置/协议边界，测试 Reset 有界结束与 key owner 不负数。
3. 性能从 source input→normalized→engine→submitted 定义时间点，注明 intentional delay。
4. 更新兼容矩阵、docs/STATUS、限制说明与迁移实例，保持 UI/CLI/文档一致。
5. 升级依赖/source baseline 时核验 manifest、重跑相关 golden；不批量覆盖 expected 去消除失败。

首发完成条件：目标功能所有行为测试通过，Linux/Windows 各自的实际发布范围有真机记录、恢复/卸载通过；没有“已支持”但状态栏写未验证的关键功能。
