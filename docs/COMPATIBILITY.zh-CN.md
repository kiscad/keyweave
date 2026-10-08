# 当前配置兼容范围

此表描述已实现程序，设计文档中的 C1/C2/C3 是开发目标。离线支持不能证明系统后端、设备抓取或注入已实现。

| 项目 | 当前行为 |
| --- | --- |
| JSONC | 接受 `//`、`/* */`，保留源行号；8 MiB、serde 默认递归限额；拒绝尾逗号、重复对象成员（含 Unicode 转义等价键） |
| profile | 选择唯一 selected=true，否则第一个；检查所有 profile 的对象、selected/name 类型；只编译当前 profile，其余保留到以后重新编译 |
| simple / basic complex | HID keyboard、consumer、button；每阶段首匹配，无回流；complex 无 modifiers 时仅在没有修饰键输出 owner 时匹配 |
| 多动作 to | 1..=256 个纯键动作；前面的动作完成 down/up，最后保持到输入 up；跨阶段中间 tap 的派生输出先释放；vk_none 无输出 |
| 持键更新 | 已持键完成旧输出，之后的 down 用新 generation；Repeat 不重新执行宏 |
| 多来源相同输出 | owner 引用集合，防止他人持键被中间 tap/up 打断；ADR-0002 说明上游兼容差异 |
| 未实现动作/条件 | 禁用整条外层 rule，保留 AST 与 JSON Pointer 诊断；validate/import/replay 不允许有损激活 |
| disabled rule | enabled=false 的内容保留，不编译、不执行；未知未来动作也不执行 |
| metadata | 根、profile、complex、外层 rule 允许 title/description/maintainers/authors/metadata；任意自定义数据放 metadata；不解释这些内容 |
| 其他功能字段 | 不静默忽略。未知根/profile/complex/rule 字段明确拒绝；非空 profile parameters/virtual_hid_keyboard、捕获 policy、全局 timing parameters 目前也拒绝 |
| macOS global | 当前明确拒绝整个 global 对象；后续区分 UI 元数据与平台功能设置，不能将其一概当无影响元数据 |
| 设备/Fn | 空数组接受；非空明确拒绝；结构错误直接拒绝快照 |
| 平台能力 | offline 支持上述 HID 页；linux-evdev 声明 keyboard/consumer；windows-hook 声明 keyboard。两个后端尚无实现，声明只用于提前检查 |

尚未实现：mandatory/optional/any、to.modifiers、repeat:false、alone/after-up、held/delayed/simultaneous、条件/变量、lazy/halt/hold_down、布局/锁状态、上下文与会话动作。含这些必要字段的规则准确拒绝。所有时间为外部注入整数纳秒；同一时刻的输出以数组次序为准。

展开限额、不同来源 tap 冲突与后续动作生命周期见 ADR-0003；实现与测试状态见 STATUS。不要通过删除不支持字段生成看似可执行的导入结果。
