# ADR-0010：有界 typed variables 与非键动作生命周期

状态：已实现离线变量切片；2026-10-08。

变量是引擎本地全局状态，不依赖 ContextSnapshot 新鲜度。类型为 i64/bool/String，严格类型相等；缺失/unset 读取整数 0。数值只接受 i64 整数，拒绝上游 JSON 浮点转整数行为。支持 variable_if/unless 与 set_variable（set/unset、value/key_up_value）；expression/key_up_expression 拒绝整条外层规则。

Action.target 明确区分 Key 与 SetVariable，不用 vk_none/sentinel 伪装非键动作。非键动作可进入有序 to/alone/after 数组，前面动作立即 down/up，最后 repeat=true 保存 key_up_value 至原输入 up；repeat=false 完成轻触。动作 conditions 整批先筛选，再执行变量赋值，因此同批后续动作条件不会读到前面动作的新赋值；后续匹配阶段/物理事件会读到新值。modifier scope 只在键目标上提供，本阶段非键 modifiers 明确拒绝。

最多 256 个变量名字，name 非空且最多 256 bytes，字符串 value 最多 4096 bytes。构造/激活快照先收集所有动作和条件名并预留槽位，名称累积跨热更新，直到 Reset 后只保留当前配置名字；持键旧 up 使用原配置和值，预留不会耗尽。构造返回 Result，拒绝超限可信 IR 与不合法值。激活失败不改变当前配置或变量，普通变量赋值不引入 key owner。

正常 up 与设备移除对已执行的变量 action 完成其 key_up_value；移除不执行 alone/after 宏。Reset 清所有值与旧名字，但保留当前配置的预留槽位。这是对上游 ungrab 丢弃 manipulated binding 的明确恢复偏差，避免 scoped layer 因拔设备遗留；变量仍是全局赋值而不是 owner 集合，两个来源的 key_up_value 按真实事件顺序覆盖，没有自动栈式恢复。

持久赋值在普通 up/设备移除后保留，直到 unset、后续赋值或 Reset。默认 CLI state 不导出变量值；replay 可显式断言 expected_variables，避免正常状态/doctor 意外暴露规则变量内容。没有 expression、系统时钟变量、IPC 外部赋值、会话继承或 OS 动作执行。

验证：9 个 engine、4 个 compiler、1 个 CLI 新测试，总计 117 个 workspace tests 通过。覆盖不同类型/缺省/Unset、模式键与旧 key binding、批量筛选、repeat:false/up-only/alone/after 生命周期、热更新/移除/Reset、共享变量赋值顺序、容量/非法 IR 与全配置 condition/action 名字预算、精确诊断和默认输出不泄露。另一个测试验证赋值不重新判定已筛选末动作的 mandatory modifier scope。

来源固定于 variable.hpp、manipulator_environment_variable_value.hpp、manipulator_environment_variable_set_variable.hpp、queue.hpp、event_sender.hpp 与 manipulator_environment.hpp；其中四个新加入 manifest，共 60 个源 hash。没有修改上游测试，没有执行 macOS 环境；恢复偏差与浮点拒绝由本项目 golden 验证。
