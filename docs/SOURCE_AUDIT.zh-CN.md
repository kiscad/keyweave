# Karabiner-Elements 源码分析与移植证据

## 1. 分析基线和边界

分析对象是本地 `../../Karabiner-Elements/`，而非网络 main 的假定状态。

本文件记录 2026-10-08 的源码评估；表中的“未进行”描述该次分析，不描述后续 Rust 项目的开发进度。2026-10-10 再运行 `python3 tools/verify_sources.py`，固定 HEAD、60 个关键源码哈希及 fixture 副本均通过。后续 Windows 原生编译验证见 [Windows 验证指南](WINDOWS_VALIDATION.zh-CN.md)，项目实际实现状态见 [STATUS](STATUS.md)。

| 项目 | 本次观察 |
| --- | --- |
| HEAD | `ec2fea8940f7254a748c052a890ee9db7113d9b1` |
| describe | `beta-169-gec2fea894` |
| version 文件 | `16.3.11` |
| 初始工作区 | `git status --short` 无输出 |
| DriverKit 子模块 gitlink | `9036bc771259ff8d61341f4d1b7072b954fdb2fa`，未初始化 |
| cpm lock 子模块 gitlink | `312906cf09a5fe414d466741380eedf11309e2e1`，未初始化 |
| vendor/vendor | 有已展开的 pqrs 等 C++ 头文件 |
| 核验方法 | 静态读取核心文件、追踪事件和配置路径、抽查上游测试与平台官方文档 |
| 未进行 | macOS 构建/运行、真实设备注入、Windows/Linux 原型和性能测量 |

固定 DriverKit 版本的 README/LICENSE 远程请求失败，因此本次只确认**主程序中的客户端调用、输出类型及子模块边界**；不能声称已经分析 DriverKit 内部队列/服务协议/驱动实现或其许可证。后续 D00 可在用户提供完整依赖后补审计，这不影响用其他 OS 接口重建后端的架构判断。

本次按指定目录递归统计 code 文件（hpp/cpp/h/swift/mm，包含这些目录下的测试）与行数，用来说明分析范围和迁移体量，不作为 Rust 工期线性估算：

| 目录 | code 文件 | 行数 | JSON/JSONC 数据文件 |
| --- | ---: | ---: | ---: |
| src/apps/CoreService | 18 | 5010 | 1 |
| src/apps/ConsoleUserServer | 18 | 2854 | 3 |
| src/share/manipulator | 48 | 8682 | 0 |
| src/share/core_configuration | 14 | 3236 | 0 |
| tests | 114 | 16775 | 510 |

510 个数据文件并非 510 个独立测试用例，有 rules/input/expected 等多文件组成的案例。没有通读所有 UI/语言资源、所有 vendor 源码或每一条 fixture；审计重点是移植关键路径。

`source-manifest.json` 保存关键文件 SHA-256、基线和分析状态。源码链接为相对本地路径，定位函数见下表；未来 checkout 变更后重新验证，不机械沿用行号。

## 2. 关键证据索引

| 编号 | 源码与定位 | 得出的结论 |
| --- | --- | --- |
| E01 | [device_grabber](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/device_grabber.hpp)，构造器与 `manipulate` | 五个处理阶段、六个事件队列、最终提交与 deadline 再调度 |
| E02 | [device entry](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/device_grabber_details/entry.hpp)，`async_start_hid_device_events_monitor` / `needs_to_seize_device` | 物理设备按需求 seize，虚拟设备只 observe；不是全设备全事件 Hook |
| E03 | [HID monitor](../../Karabiner-Elements/src/share/hid_device_events_monitor.hpp)，构造器 / `normalize_time_stamps` | 统一 IOHID value 与特定 raw report 恢复事件，维持时间戳一致性 |
| E04 | [event entry](../../Karabiner-Elements/src/share/event_queue/entry.hpp) 与 [timestamp](../../Karabiner-Elements/src/share/event_queue/event_time_stamp.hpp) | device、original_event、validity、state、lazy 与 input delay 都是语义数据 |
| E05 | [event queue](../../Karabiner-Elements/src/share/event_queue/queue.hpp)，`emplace_back_entry` / `sort_events` / `needs_swap` | 入队更新修饰/变量/上下文；同时间戳修饰 down 优先、up 延后 |
| E06 | [manager](../../Karabiner-Elements/src/share/manipulator/manipulator_manager.hpp)，`manipulate` / `remove_invalid_manipulators` | 有效事件才透传；等待会暂停；invalid 且 inactive 才移除 |
| E07 | [connector](../../Karabiner-Elements/src/share/manipulator/manipulator_managers_connector.hpp)，`connection::manipulate` | 顺序连接阶段，每阶段处理直到无进展，不是一个全局 map |
| E08 | [basic](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/basic.hpp)，`manipulate` / `unset_alone_if_needed` | 按下创建原始绑定、按松开查绑定；组合键缓冲；alone 严格时间边界 |
| E09 | [event sender](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/event_sender.hpp) | 输出序列、lazy 修饰、mandatory 修饰释放/恢复、key_up 动作需要共同移植 |
| E10 | [virtual output](../../Karabiner-Elements/src/share/manipulator/manipulators/post_event_to_virtual_devices/post_event_to_virtual_devices.hpp) 与 [queue](../../Karabiner-Elements/src/share/manipulator/manipulators/post_event_to_virtual_devices/queue.hpp) | 系统输出适配与规则状态分离，包含 HID reports 与用户动作 |
| E11 | [key dispatcher](../../Karabiner-Elements/src/share/manipulator/manipulators/post_event_to_virtual_devices/key_event_dispatcher.hpp) | 普通键按 usage 去重、按 usage 松开；不等于多来源引用计数 |
| E12 | [from modifiers](../../Karabiner-Elements/src/share/manipulator/types/from_modifiers_definition.hpp)，`test_modifiers` | mandatory/optional/any 含义与左右抽象匹配 |
| E13 | [held down](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/to_if_held_down.hpp) 与 [delayed action](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/to_delayed_action.hpp)，`setup/cancel` | 伪时间可测的任务与取消；不能用简单 elapsed 判断代替 |
| E14 | [simultaneous options](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/simultaneous_options.hpp) 与 [from definition](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/from_event_definition.hpp) | strict/inverse、any/all、uninterrupted 等必须显式支持或诊断 |
| E15 | [other key pressed](../../Karabiner-Elements/src/share/manipulator/manipulators/basic/to_if_other_key_pressed.hpp) | 新基线支持其他按键触发输出替换，应列入兼容清单 |
| E16 | [manipulator factory](../../Karabiner-Elements/src/share/manipulator/manipulator_factory.hpp) 与 [condition factory](../../Karabiner-Elements/src/share/manipulator/condition_factory.hpp) | 不只有 basic，还有鼠标规则及 expression、event_changed、device_exists 条件 |
| E17 | [configuration](../../Karabiner-Elements/src/share/core_configuration/core_configuration.hpp) 与 [monitor](../../Karabiner-Elements/src/share/monitor/configuration_monitor.hpp) | JSONC、owner/size、user/system/default 选择；错误不发布；删除延迟 1 秒 |
| E18 | [rule](../../Karabiner-Elements/src/share/core_configuration/details/profile/complex_modifications_rule.hpp)、[assets](../../Karabiner-Elements/src/share/complex_modifications_assets_file.hpp)、[Duktape](../../Karabiner-Elements/src/share/duktape_utility.hpp) | eval_js 和 .js 生成规则存在，禁用 JS 规则不执行，不能忽略这层 |
| E19 | [parameters](../../Karabiner-Elements/src/share/core_configuration/details/profile/complex_modifications_parameters.hpp) | 50/1000/500/500 ms 默认，规则局部参数来自复制并覆盖 |
| E20 | [device code stage](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/device_grabber_details/device_key_code_manipulator_manager.hpp) 与 [simple stage](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/device_grabber_details/simple_modifications_manipulator_manager.hpp) | ISO 键交换通过 basic 保存生命周期；设备 simple 优先于全局 |
| E21 | [Fn stage](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/device_grabber_details/fn_function_keys_manipulator_manager.hpp) | 依赖 macOS Fn 偏好，并用 event_changed 避免覆盖已修改事件 |
| E22 | [event tap](../../Karabiner-Elements/src/share/monitor/event_tap_monitor.hpp)，`callback` / `should_skip_keyboard_event` / `async_stop` | 回退缺设备身份，有注入防循环、重复识别、线程停止协议 |
| E23 | [suppression](../../Karabiner-Elements/src/share/keyboard_suppression.hpp) 与 [loop guard](../../Karabiner-Elements/src/share/keyboard_fallback_loop_guard.hpp) | 短时事件队列与快速循环保护不同；CapsLock 有特殊匹配 |
| E24 | [daemon components](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/components_manager.hpp) 与 [receiver](../../Karabiner-Elements/src/apps/CoreService/include/core_service/daemon/receiver.hpp) | 当前 console user 变化重建 socket；权限和连接状态参与 grabber 生命周期 |
| E25 | [ConsoleUserServer](../../Karabiner-Elements/src/apps/ConsoleUserServer/include/console_user_server/components_manager.hpp) 与 [shell handler](../../Karabiner-Elements/src/apps/ConsoleUserServer/include/console_user_server/shell_command_handler.hpp) | 用户上下文与 /bin/sh 命令独立于设备处理，Rust 要保留权限隔离 |
| E26 | [momentary switch](../../Karabiner-Elements/src/share/types/momentary_switch_event.hpp) | 核心键以 HID usage page/usage 表达，含 Apple vendor 扩展 |
| E27 | [frontmost condition](../../Karabiner-Elements/src/share/manipulator/conditions/frontmost_application.hpp) 与 [input source condition](../../Karabiner-Elements/src/share/manipulator/conditions/input_source.hpp) | bundle/file regex、macOS 输入源字段都要平台适配 |
| E28 | [test helper](../../Karabiner-Elements/tests/src/share/manipulator_helper.hpp)、[fixture 目录](../../Karabiner-Elements/tests/src/manipulator/json) | pseudo time、规则与输入/expected 数据、invalidate 操作能作为差分基础 |
| E29 | [tests build](../../Karabiner-Elements/tests/tests.cmake)、[CoreService project](../../Karabiner-Elements/src/apps/CoreService/project.yml)、[vendor dependencies](../../Karabiner-Elements/vendor/CMakeLists.txt) | C++23、Xcode/Apple framework/pqrs::osx 链路，不能直接在 Linux 链接原引擎 |
| E30 | [README](../../Karabiner-Elements/README.md)、[LICENSE](../../Karabiner-Elements/LICENSE.md)、[gitmodules](../../Karabiner-Elements/.gitmodules) | 主项目 Unlicense 与独立子模块/预构建依赖需分别核验 |

## 3. 输入路径：为何简单 Hook 不够

### 3.1 主路径

`device_grabber` 创建 IOHID manager，匹配 keyboard/mouse/pointer/joystick/gamepad/consumer 等 usage，device matched 后创建 entry 并接入事件信号。entry 保存设备 properties、pressed keys、LED manager 等局部状态。

`make_grabbable_state` 先允许被观察的虚拟设备，再确认真实设备需要 seize；虚拟键盘未准备好、需要指针但虚拟指针未就绪时，不独占输入。这个检查直接防止“拦截成功但没有输出路径”导致键盘失效，是跨平台不可遗漏的恢复原则。

entry 使用 `kIOHIDOptionsTypeSeizeDevice` 独占真实目标。disabled 的内置设备仍需 seize 才能丢弃输入；ignore 则不进入映射。设备消失/monitor stopped 后移除物理按住集合，并送 device_ungrabbed 到流水线，输出阶段负责释放。

`hid_values_arrived` 只将 seized 且未 disabled 的真实设备事件送入 merged queue；重复 down/无对应 down 的 up 被物理 pressed 集合筛选。虚拟设备观察主要用于 CapsLock LED 状态，不回送普通键输出形成环。

### 3.2 raw report 补充

`hid_device_events_monitor` 在 IOHIDQueue 的 value 路径外，对 registry 中识别的特定设备解析 descriptor/report。当前 registry 有 Elecom trackball 相关处理，目的是恢复 macOS 不经 IOHIDElement 暴露的事件。

这不是所有设备的统一 raw HID 解析器。Linux 多数需要键已经由内核转成 evdev；Windows 相应 consumer/vendor interface 需另做覆盖调查。不要把这个 macOS workaround 当作 Rust 项目必须立即重写的基础设施。

### 3.3 CGEventTap 回退

默认 EventTap 观察 Apple trackpad 指针事件，辅助 alone 等状态；开启 keyboard fallback 后，也捕获 keyDown/keyUp/flagsChanged/NX_SYSDEFINED。回调在合适情况下返回 nullptr 消费键盘事件，再异步送同一规则管线，虚拟 HID 输出仍是输出通道。

回退输入采用 `device_id(0)`：没有真实设备身份。它可与主路径共存，不能笼统理解为“完全替换 HID seize”。输出经 EventTap 被观察回来，通过短期 suppression 避免再次处理；另有 loop guard 限制快速循环。

CapsLock suppression 的 200 ms 是经验余量，有延迟/无 CGEvent/真实相同按键误消费等边界。这个具体 TTL 不适合作为 Windows 来源识别策略。源码还同步指针事件的虚拟修饰 flags、归一 Fn 修改的导航键、用虚拟 pressed keys 判断 repeat 透传，体现了高层 Hook 与低层 HID 不等价。

secure event input 打开时，IOHID 路径仍可收事件而 EventTap 可能失去 up，因此 fallback 清物理状态并释放虚拟输出。停止 EventTap 也有专门生命周期：disconnect、在自身 run loop 清理、stop、异步 completion、join；不能仅销毁引用等待 Rust RAII 自动解决回调竞态。

## 4. 规则引擎：可重写，但要按状态机实现

### 4.1 事件不止 key_down/key_up

queue entry 保存 `original_event` 和当前 event，区分 `state::original/manipulated/virtual_event`，并携带 validity、lazy、integer value。`emplace_back_entry` 同时更新 modifier/button manager 和 manipulator environment 的设备、应用、输入源、变量、系统偏好与虚拟设备状态。

因此“队列里的事件处理”和“队列状态改变”是联系在一起的。Rust 可以优化为 VecDeque、索引和紧凑状态，不需要复刻 C++ 容器/所有 mutex，但要保留观察到的顺序语义和阶段状态。

### 4.2 匹配、消费、配对和多阶段

manager 逐事件、逐 manipulator 处理。被 manipulator 标 invalid 的输入不会在该阶段最终透传；already_manipulated 防止 simultaneous 的后续原 down 再次消费。needs_wait_until_time_stamp 暂停输入，而 connector 仍按固定阶段顺序推进其他可推进部分。

basic 在 down 检查 from、修饰、整数值和条件，建立 `manipulated_original_event`；up 用 device+current/original event 找原记录，避免配置或条件变化导致松错键。输出动作暂时释放 mandatory 修饰，生成 to，按末尾输出是否是修饰键决定恢复时机。

simple stage 同样使用 basic，因此可以继承 down/up 和 invalid/active 的生命周期，不应该在 Rust 输入归一层直接使用新 map 重写 up。

### 4.3 时间行为的细节

simultaneous 在前事件时间+threshold 内扫描队列，只采每个不同事件的第一份；如果另一个 down 打断且未配置 uninterrupted 则取消；来源可以不同设备。strict up order 可能还需要继续等候。等待时更新 input_delay，必要时发 stop_keyboard_repeat，超时继续判定/回放。

alone 在 `get_alone && duration < timeout` 时执行。其他 momentary switch down、非零横/纵滚轮使已有 binding 不再 alone；单纯 pointer move 不做该取消。held_down 和 delayed_action 都有 debounced task；任务需 lock 活跃绑定/输出 queue 才继续，配置 invalid 与设备清理还影响其存活。

`to_if_other_key_pressed` 为原绑定保留状态，匹配后释放原输出并建立替代输出，原来源 up 时清替代状态。这说明“常见 tap/hold、layer 实现了”不能替代当前源码的全部功能盘点。

### 4.4 更新与输出账本

热更新 invalidates manipulators，重新按顺序建新对象。remove_invalid_manipulators 只删 invalid 且 inactive 的对象，所以旧 up 仍有对应处理者。`device_grabber` 的设备键码交换注释也明确保存 down 选中的映射直到 up。

输出阶段将逻辑事件变为 keyboard、consumer、Apple vendor、generic desktop、pointing reports 以及 shell/input-source/software-function 等队列项。`key_event_dispatcher` 维护已发普通键与修饰集合，避免重复 down，并能按 device ID 清理。

普通键 dispatcher 对 usage 去重，只保存首次按下的设备，不是 `(device, usage)` 的完整 owner set。Rust 项目建议改善这一点；保持修饰跨设备语义的同时为普通键做 owner tracking。对新算法做独立测试与偏差说明，而不是复制上游限制后声称天然安全。

## 5. 配置、条件和用户会话

主配置支持 JSONC，最大文件 64 MiB，限制 owner 为 root/预期用户。user 文件存在时优先使用；缺失才考虑 system/default。权限错误不是不存在，避免意外切换配置。monitor 对已载入文件短暂删除去抖 1 秒，parse error 发诊断但不发 updated。

configuration details 通常保留原 JSON 再更新已知项；basic/from/to parser 对功能字段严格程度更高。Rust 不能统一 `deny_unknown_fields` 导致资产元数据全报错，也不能统一忽略未知字段导致条件消失。建议 AST 保留、IR 严格、规则级能力诊断。

condition factory 有 device_if/unless、device_exists_if/unless、event_changed、expression、frontmost_application、input_source、variable、keyboard_type。表达式 manager 与变量更新相联系。变量不只是整型层号，还需要处理值类型、unset、key_up_value 和表达式形式。

复杂 rule 可以由 eval_js 解析得到 JSON；禁用 JS 规则保留源码且不执行，启用时先构造成功才激活。资产加载接受多规则 JSON/JSONC、单规则文件和 .js。Duktape 有独立大小、内存和超时约束。这些应该进入 Rust 兼容诊断与后期计划，首版不执行并非可静默丢弃。

CoreService daemon 的组件随着 console uid 更换 receiver，并改 socket owner；ConsoleUserServer 监听当前用户是否在 console、等待 daemon server 绑定后再连接，处理应用/输入源/偏好/UI 与用户动作。root 映射进程和用户动作进程是架构边界，不只是 GUI 包装。

shell handler 明确 `/bin/sh -c`，迁移到 Windows 无法直接复用。前台条件匹配 bundle identifiers/file paths 的 regex_search，macOS 语言/input_source_id/input_mode_id 也需平台 provider。SwiftUI 设置与 EventViewer 不宜翻译，而是重建共享 Rust 控制面和 GUI。

## 6. 可复用测试与代码复用取舍

上游有 parsing/type 单元测试，也有 manipulator rules/input/expected_event_queue/expected_post_event_to_virtual_devices_queue 的回放数据。`tests/src/share/manipulator_helper.hpp` 替换 dispatcher 的 time source，推进伪时间、控制 pause、invalidate、比较队列。这能直接指导 Rust replay driver。

注意 helper 额外构造测试用 Fn stage（例如 F10→mute 并检查 event_changed），不是所有生产配置的统一预设。JSON enum 值和 Mach ticks 是源实现编码，先转成明确 KeyWeave fixture 模型，再对比语义。

适合复用：键名/usage 数据、配置外形、规则与时间语义、合成 fixture、诊断案例与用户体验思路。需保留出处与许可证。

适合重新实现：所有状态结构与容器、dispatcher/timers、输出所有权、配置编译、IPC、OS backend、service manager、GUI。Rust ownership 能减少悬空回调与共享状态，但不会自动解决错误键码、错误时序、死锁、输出失败或驱动 IRQL。

不建议将 C++ 引擎永久包 FFI 作为主方案：核心头文件经 types/event_queue/manipulator environment 等依赖 pqrs::osx、Mach time、Apple input source/偏好与 virtual HID 类型；跨平台抽离本身就是较大重构，又违反核心 Rust 重写目标。Mac-only oracle 可单独保留，不进入最终运行依赖。

## 7. 移植风险排序

| 优先级 | 风险 | 必须完成的验证 |
| --- | --- | --- |
| 最高 | 拦截后无法输出、输出 key 卡住 | 输出 ready→抓取次序、故障注入、独立恢复路径 |
| 最高 | Windows Hook 被当作按设备过滤器 | 能力拒绝、禁止 RawInput/Hook 时戳猜关联 |
| 最高 | 驱动线程等待用户态引擎 | 预分配、有界队列、回调不阻塞、租约恢复 |
| 高 | timer/热更新/重插沿用旧 ID | token 与 generation、边界和 Reset replay |
| 高 | 规则条件不支持而被丢弃 | 整条禁用、JSON Pointer 诊断、导入不执行 |
| 高 | 多来源同键提前松开 | owner set、临时修饰账本、独立偏差 golden |
| 高 | 锁屏/多会话用户动作跨权限 | 当前会话租约、未知状态暂停、不以 root 执行 |
| 中 | 标准键名与布局/IME 混淆 | HID/native 映射表、ISO/JIS、特殊扫描码实测 |
| 中 | Wayland 全局上下文能力不存在 | provider 探测、Unknown 三态、不夸大覆盖 |
| 中 | 全 Rust 驱动生产工具链与签名 | 独立 D10 gate，用户态版不受其阻塞 |

对应解决方案、阶段和验收标准已落实到开发指导与任务清单；这些风险是具体实现要求，不是“移植不可行”的结论。
