# 当前配置兼容范围

此表描述已实现程序，设计文档中的 C1/C2/C3 是开发目标。离线支持不能证明系统后端、设备抓取或注入已实现。

| 项目 | 当前行为 |
| --- | --- |
| JSONC | 接受 `//`、`/* */`，保留源行号；8 MiB、serde 默认递归限额；拒绝尾逗号、重复对象成员（含 Unicode 转义等价键） |
| profile | 选择唯一 selected=true，否则第一个；检查所有 profile 的对象、selected/name 类型；只编译当前 profile，其余保留到以后重新编译 |
| simple / basic complex | HID keyboard、consumer、button；每阶段首匹配，无回流；complex 无 modifiers 时仅在没有修饰键输出 owner 时匹配 |
| 多动作 to | simple 1..=256、complex 0..=256 个键动作；前面的动作完成 down/up，最后默认保持到输入 up；repeat:false 使最后动作也立即轻触；中间 tap 的派生输出与 up 宏先结束；vk_none/空 complex to 无输出 |
| from.modifiers | mandatory/optional/any；八个 HID 左右键与 command/control/option/shift 组；组左优先，允许整组额外侧；optional 保留，mandatory 临时屏蔽；接受字符串/数组、别名去重；caps_lock/Fn 明确拒绝 |
| to.modifiers | 标准左右修饰与抽象组（左侧）；字符串/数组（最多 64 名），去重并按上游 modifier 枚举顺序；any 无动作；modifier/key 均进入下一阶段；普通主键 scope lazy up，末主键是 modifier 则 scope 持有到原 up |
| to_if_alone / to_after_key_up | 键与修饰作用域完整轻触；正常 up 先释放主动作，再 alone，再 after；alone 严格小于超时，新 key/button down 和非零 wheel 取消，move 不取消；lazy 恢复 down 时八个 HID 修饰键；拔除/Reset 取消宏 |
| to_if_held_down | 阈值先释放正常 to，再执行 held；最后输出持有至原 up；当前动作条件+原配置/设备；其他新 key/button 或同阶段派生 down 取消未触发任务，move/wheel/repeat 不取消；held 不自动取消 alone |
| to_delayed_action | complex/basic 的 to_if_invoked/to_if_canceled（对象或数组），默认 500ms；up 后继续等待；新 key/button 或同阶段派生 down 先执行 canceled；到期 invoked；两者单次完整轻触，不替换普通 to；保留旧配置/down 修饰快照与当前动作条件 |
| to.lazy | 布尔值，默认 false；八个标准修饰键 down/up 只更新逻辑 owners，后续普通 down/显式修饰/允许的指针事件/设备 neutral 刷新；非修饰键和变量赋值正常执行；下一阶段重映射采用其动作策略 |
| to.halt | 布尔值，默认 false；当前动作数组继续，已安排的 up 和变量 up 值完成；同一 rule binding 后续 held/alone/after/delayed 批次停止；被 conditions 过滤的动作无作用；各阶段 binding 独立 |
| pending 与取消 | up 后的任务保留 pending binding/cursor/reservation；最后任务完成回收；Reset/remove/代际替换直接撤销无宏；大取消宏+新 down 超预算或需回收 pending 容量时 CancellationPending，按 next_delayed 单独 CancelDelayed 后重试 |
| deadline 控制 | 显式 TimerFired{token}、Engine.next_deadline 时间/token；阈值时 timer 在普通物理事件前，Reset/remove 优先取消；提前/乱序拒绝，stale 无输出；晚交付按实际时间执行；单次一个任务，ADR-0011 |
| 设备归零 | 已知物理 up 后该设备不再有 binding 时提交 pending modifier 状态；未知 up 不推断归零；拔除结束后也刷新；不等同于 OS lost-input/submitted 故障恢复 |
| basic timing parameters | 四个已知时间参数整数 0..=86400000ms；默认 alone=1000ms，支持规则 override；alone、held、delayed 有执行行为（held/delayed 默认 500ms），simultaneous 参数保存待规则字段实现 |
| 持键更新 | 已持键完成旧输出，之后的 down 用新 generation；Repeat 不重新执行宏 |
| 多来源相同输出 | owner 引用集合，防止他人持键被中间 tap/up 打断；ADR-0002 说明上游兼容差异 |
| 设备代际 | 每个 id 仅保留最高 generation/removed；旧移除幂等忽略，旧 key/wheel/move 拒绝；4096 身份上限，Reset 保留历史；ADR-0005 描述容量恢复契约 |
| 应用条件 | frontmost_application_if/unless；bundle_identifiers/file_paths regex_search OR；没有标题匹配；provider 属性缺失/非 ASCII/CRLF/过期 Unknown |
| 输入源条件 | input_source_if/unless；input_sources 字段 language/input_source_id/input_mode_id AND、条目 OR；空 specifier 在已知源对象中 True |
| 变量条件 | variable_if/unless；严格 i64/bool/String 相等；未设置/unset 为整数 0；引擎本地状态不受 ContextSnapshot 过期影响 |
| 变量动作 | complex to/alone/after 中 set_variable，默认 type=set；value/key_up_value 至少一个，type=unset 不接受被忽略的 value；最后 repeat=true 的 up 值保留到原输入 up；非最后/extra/repeat=false 完整 down/up；没有按键 owner |
| 变量预算与恢复 | 256 names、256 bytes/name、4096 bytes/string；构造/激活预留，跨热更新保留，Reset 清值并回收旧配置名字；移除完成已执行 action 的 up 值，保留普通持久赋值；全局按顺序覆盖，ADR-0010 |
| 正则 | 仅 ASCII 单行匹配；配置时预编译，最多 256 patterns/当前配置、1024 bytes/pattern；32 KiB 程序/16 KiB DFA cache；拒绝 lookaround/backreference/inline flags/Unicode 或 hex 转义、集合运算、嵌套字符类/类内 shorthand；不是完整 C++ ECMAScript，ADR-0009 |
| 设备条件 | device_if/unless/exists_if/exists_unless；identifier 字段 AND、列表 OR；空对象忽略；规则数组全部 True；输出 action.conditions 在执行时筛选；未知/过期对 if/unless 均不激活，ADR-0008 记录上游差异 |
| 上下文输入 | ContextChanged：严格 revision、provider、[observed, valid_until) 纳秒；一天有效期上限；256 项设备库存、每 id 唯一；未知属性用 None；设备重连使 metadata 失效；可携应用/输入源属性，只支持离线合成 provider |
| 未实现动作/条件 | 禁用整条外层 rule，保留 AST 与 JSON Pointer 诊断；validate/import/replay 不允许有损激活 |
| disabled rule | enabled=false 的内容保留，不编译、不执行；未知未来动作也不执行 |
| metadata | 根、profile、complex、外层 rule 允许 title/description/maintainers/authors/metadata；任意自定义数据放 metadata；不解释这些内容 |
| 其他功能字段 | 不静默忽略。未知根/profile/complex/rule 字段明确拒绝；非空 profile parameters/virtual_hid_keyboard、捕获 policy、未知 timing parameters 目前也拒绝 |
| macOS global | 当前明确拒绝整个 global 对象；后续区分 UI 元数据与平台功能设置，不能将其一概当无影响元数据 |
| 设备/Fn | 空数组接受；非空明确拒绝；结构错误直接拒绝快照 |
| 平台能力 | offline 支持上述 HID 页；linux-evdev 声明 keyboard/consumer；windows-hook 声明 keyboard。windows-hook 拒绝设备条件；两个后端尚无实现，声明只用于提前检查 |


尚未实现：simultaneous、expression/系统变量、hold_down_milliseconds、布局/锁状态、真实上下文 provider、Unicode 条件正则与会话动作。含这些必要字段的规则准确拒绝。所有时间为外部注入整数纳秒；同一时刻的输出以数组次序为准。wheel/move 当前只有离线 Pass、取消与受 repeat/button 状态约束的修饰刷新，没有指针捕获/映射后端。隐式 mandatory/scope 修饰恢复见 ADR-0006/0007，显式 lazy/halt 的状态与预算见 ADR-0013。

展开限额、不同来源 tap 冲突见 ADR-0003；轻触/松开生命周期与预算见 ADR-0004；实现与测试状态见 STATUS。不要通过删除不支持字段生成看似可执行的导入结果。

上下文 JSON 原文不进入 CLI 默认输出。已持键的输出释放不重查条件；alone/after-up 的动作条件读取执行时上下文，但采用原 binding 保存的配置。条件过滤后最后一个有效输出决定是否保持按下。设备地址、location 等缺失时保持 Unknown，is_touch_bar 明确拒绝。ContextChanged 不代替可信系统 provider、session lease 或故障恢复。

变量浮点/超 i64/null/对象、expression/key_up_expression、set_variable 与另一个目标同置、非键 modifier scope、simple 非键目标都拒绝。动作 conditions 在同一 to 数组执行前整批筛选；变量赋值可影响随后阶段的规则匹配及后续物理事件。CLI fixture 可给 expected_variables（需 --assert），输出及变量断言失败消息不导出实际值。

held 展开、future macros/suppression/deadline token 在原 down 一并预留；超限/时间溢出在改变输出前拒绝。上下文更新可先于同刻 timer，以便使用新属性；普通 key/wheel/move 遇到未交付到期 timer 会返回 DeadlinePending，运行时先按顺序 fire，再提交原事件。纯 held 规则可以没有普通 to。

延迟任务 active+pending 总量受同一 4096 binding/16384 reservation 上限保护；设备 idle 后延迟宏完成会刷新 latent modifiers，此恢复选择见 ADR-0012。EngineState.pending_bindings 描述松开后等待的任务，0 时省略序列化；replay next_deadline 给出 at_ns/token。CancelDelayed 不可抢先绕过已到期 timer；未知旧 token 无输出。按住/延迟仅属于 complex/basic，直接构造的 simple timed IR 也拒绝。单对象动作的诊断 pointer 指向对象成员，数组才含 /索引。

变量 down 会同步逻辑修饰状态；lazy 不延迟赋值。输出边沿与 owner/token 预留独立计算，包含最多八个 pending modifier 边沿，超过 1024 在 down 前拒绝；配置编译也检查此上界。halt 不撤销已安排 deadline，回调仍结束其 binding 生命周期并回收预算。

Linux 原生转换组件已有 159 HID/native 对与 154 canonical codes，但未接入 validate/backend；粗粒度 linux-evdev 页声明不能证明任意 HID 输出可用。keyboard 音量、non_us_pound/clear 等五个 native 别名明确拒绝保真输出；canonical evdev 身份不证明原始 HID usage，可信 scan 需 driver/descriptor 核验。详细策略见 ADR-0019。尚无真实设备抓取/注入。

Windows 后端进展：已有 portable scan codec、默认 injected bypass、保存 down identity 的 normalizer，以及处理部分插入/显式停捕获恢复的 OutputPump。cfg Windows 原生 SendInput 已编写，已在宿主 Windows 固定 Rust 1.97.1 通过 x64/i686 编译/生产链接，x64/x86 SDK ABI oracle 通过；真机验证待执行；已有同步决策绑定和原生 HookSession 注册/消息循环/卸载代码；无完整可执行会话运行时，CLI Windows capability 仍是规划的配置检查范围，不代表可安装运行。特殊 scan/别名输出在 backend 验证中拒绝。SendInput 的全局状态不会随 backend Drop 清除；未知接受量保留 quarantine，未捕获物理键碰撞协调仍待实现（ADR-0032/0033）。

Windows SessionActor 已连接共享引擎与输出账本，ActorHandler 可交付同步 HookHandler；覆盖标准键捕获重发、持键热更新与显式 tick 推进时间任务。Engine 物理 Repeat 不重播宏；Windows actor 已按原来源当前保留末端发送一条 Repeat，不增加 owner或重播前缀/变量，热更新读旧 binding、held 到期先换目标。repeat:false/已结束/非键盘/标准修饰不重复；采用当下已 emitted 的修饰状态，不重建首次 shortcut scope（ADR-0042）；已有具体同 epoch SessionClock/SessionControl 与原生定时 poll_with_control 源码；Windows 实际等待未验证；已有 passive neutral 启动组件，已有映射前固定紧急组合；碰撞/监督与可执行启动尚未完成，不能视为可运行 Windows 后端（ADR-0035）。

Windows startup 现有 callback 外只读采样：visible WinSta0/input desktop/HOOKCONTROL 与 foreground integrity 访问检查、无 impersonation、254 VK high-bit aggregate、pre/post context 重验。返回 Held/IdleObserved，不自动启用；不是原子 physical neutral。CaptureSession 已实现 passive Hook barrier/启动与关闭事务：启动 2s 期限、采样前后全部 binding 为空才同步启用，正常 capture 不全 VK 扫描；错误/Quit 先 close Hook 后有限恢复，保存卸载/恢复诊断；Drop 仅停 Hook，不注入或保证清全局键。实际 Windows API 与可执行 host 验证继续待做（ADR-0037/0038）。


Windows CaptureSession 紧急退出：原 physical Ctrl+Alt+Shift+F12 左右任一持续 >=1s，injected/passive 不参与，重映射不可覆盖。检测与 actor 共用一次 fresh delivery time，紧急到期先于同刻 up、held/delayed timer 和 due output；无新输入也在 control 推进。poll 返回 Emergency 后已关闭 Hook 并有限恢复，调用者检查 shutdown_report 的卸载/恢复结果；失败或未知 receipt 不宣称已清按键。Hook 无可信逐事件 keyboard ID，因此此组合按合并流检测，允许跨键盘组成。线程停滞/Hook 移除/桌面切换不受该检测覆盖，独立监督与完整 host 仍未完成（ADR-0039）。


Windows 活动访问检查已接入 CaptureSession：100ms 到期在 callback 外仅复核桌面/访问权限与前台 integrity，零 GetAsyncKeyState 调用；成功按完成时刻续期 500ms lease，callback 与 control 同 delivery time 检查有效期。高 integrity、失去输入 desktop、API/close/metadata 错误或过期/迟到结果终止会话并先停 Hook 再恢复，正常成功检查保留持键绑定。query 期间 callback 已停止时，不再续期/推进 actor。metadata pre/post 不原子，变化至下一检查之间仍可发生访问竞态；参数不提供墙钟时限，不能检测 Hook 静默移除/线程卡死，完整 WTS/session disconnect 与独立监督仍待实现（ADR-0040）。


当前 Windows 编译证据：宿主 Windows 10 build 19045 / Rust 1.97.1 / MSVC 19.44.35229.0 / SDK 10.0.26100.0，x64/i686 各 workspace 432 tests、fmt/clippy、native_link 和12个离线回放通过，SDK x64/x86 compile-only oracle 通过。MSG 的 `_MAC` 保留字段已删除，Windows size48/28。生产符号链接不执行 Hook/SendInput；独立监督、完整可执行 runtime 与真实设备语义/故障仍待完成（ADR-0041；[固定版本证据](validation/windows-2026-10-09-pinned.json)）。


Windows 未归属输入保护（ADR-0043）：active会话在other injected、unsupported physical Pass或故障放行down/repeat时停止；OwnInjected仍bypass，marker+flag不是鉴权凭据。风险sticky标记独立于首个错误；发送重入后禁止后续批次再调用backend。若有submitted down或unknown，不发可能释放外部来源的up，ShutdownReport.untracked_input=true且Unresolved；已知空ledger则Stopped。故障导致的有效captured up不会凭空建立新down，仍可走正常有限恢复；fault后close前观察到额外旁路会再次标记风险。此层会使外部自动化/unsupported键终止当前active会话，不能视为已支持共存或已完成逐键碰撞；unhook后的输入、系统静默移除及close/recovery窗口仍需完整监督/协调和真机验证。


Windows WTS租约检查（ADR-0044）：本地caller与foreground所属session必须相同且非0/保留值，WTSSessionInfoEx level1/完整长度、Active+Unlock和有效LogonTime均要求。Context两次比较包含session identity；CaptureSession也比较startup基线，已有新active session观察不能续旧会话。锁定/断连/身份变更/未知/权限或服务失败停止并沿用close-first恢复或untracked Unresolved规则。WTS所有操作在callback外，租约100/500ms不是RPC时长保证；Windows10/11目标，Win7反转flags不支持。尚无WTS事件通知/独立监督，检查间隔与pre/post非原子竞态保留；不能宣称真实锁屏/RDP/注销验收通过。

Windows WTS通知在本session任何变更时停止，包括unlock/connect/desktop-ready，不能由消息授予或恢复lease。另session消息忽略；通知不认证发送者且有交付延迟。通知发生后保留非空/unknown submitted，禁止recovery新提交；通知前已进行的API仍按真实receipt确认，不宣称跨桌面原子性。没有独立监督或真机验收；metadata查询失败且尚无通知时也通过ADR-0046的独立锁存阻止恢复提交；没有跨桌面原子性。见ADR-0045/0046。

Windows metadata或短租约失效，即使WTS消息没有到达也阻止新恢复提交；known empty正常停止，nonempty/unknown submitted保留为Unresolved。later成功诊断不恢复旧lease；secondary probe错误不被首callback故障遮蔽。通知到达/观测返回之前的实际API仍按receipt确认，不能宣称无窗口的原子停止。见[ADR-0046](ADR-0046-windows-revoked-access-recovery.md)。

进度监督v1协议由共享crate统一，Linux阶段enum保持兼容别名及原字节值。Windows passive/Standby的实际poll会增加计数，但不回应进度；publish须看到challenge之后完成的poll，并且phase已进入Active/Recovering/Stopped。Stopped包含Unresolved，只表示事件循环停止，不证明全局按键已释放或进程退出。客户端拒绝callback IPC；generic streams必须由host提供真实非阻塞端点，当前未提供Windows native监督器。见ADR-0047。


Windows新增owned Job进程资源接口（ADR-0048），无设备/输入行为：挂起创建、private Job分配、显式恢复、停止请求和实际退出分别表达；JobSpawnError保留partial owner。双架构无输入原生例程已确认argv传递、259退出和own Job显式终止。它尚未接入进度监督或host；Create→Assign期间的父崩溃窗口、真实父死亡与Hook存活仍待实现/验收，不能据此宣称卡死/崩溃恢复已完成。


ADR-0049把Windows owned Job准入放入CreateProcessW扩展启动事务，无创建后Assign回退；创建成功时worker已在private Job内。Windows x64/i686实际无输入fixture已验证父被强制终止后worker退出（通过保留进程句柄观察，不靠Rust Drop/PID消失）。该结果不证明全局注入键清理、Hook存活或完整进度监督；native IPC/启动核验/host仍缺失。


Windows新增native overlapped pipe transport（ADR-0050）：私有单向byte pair、64-byte稳定buffer/单flight/不可继承句柄，与共享v1协议相容；pending不等于发送完成，changed pending write拒绝。显式取消只有event completion确认后才能回收；Drop未完成会保留资源并毒化线程，不承诺全部回收。双架构无输入原生例程验证same-process往返/EOF/cancel；fixture epochs不表示真实runtime活性，尚无child端句柄交付/启动鉴别或独立progress supervisor。
