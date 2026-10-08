# ADR-0006：修饰组匹配与按来源临时屏蔽

状态：八个标准 HID 修饰已实施；2026-10-08。

依据：固定上游 from_modifiers_definition.hpp / modifier_definition.hpp，mandatory:any 返回全部当前修饰且忽略其他限制；普通抽象组选择第一个当前按住的左右键（左优先），严格检查额外修饰时允许整组。optional:any 保留未消费修饰；JSON 数组顺序不改变选择。八个标准 HID 左右修饰与抽象 command/control/option/shift 支持，caps_lock/Fn 锁或 vendor flag 暂不支持并诊断。

选择：临时屏蔽被消费键的当前可见 source owners，保留逻辑物理所有权。后续新输出 owner 不被旧屏蔽命中。lazy modifier 状态与 emitted pressed set 分开：屏蔽/恢复先改变逻辑状态，在普通键 down 或显式修饰事件上提交修饰边沿；普通 key_up 不强制刷新。末动作是修饰时屏蔽保持到规则 up，否则主动作 down 后逻辑恢复，物理恢复等待下一需要修饰提交的事件。wheel/move 在最后 emitted 普通键已 up 且无按住的按钮时可刷新；repeat detector 跟随实际输出边沿，不用所有物理 held 键代替它。

up 宏再次临时屏蔽消费修饰；alone 先恢复 down 时修饰作用域，再屏蔽 mandatory，保证消费键不会混进轻触输出。拔除释放 owner/屏蔽，不执行宏；若有实际 modifier key-up，提交当前完整修饰状态。Reset 只为 emitted pressed set 发 up。

屏蔽按可见 owner 快照建表，不对已屏蔽 owner 重复叠加。同键新来源可继续按住；旧物理来源先 up，不复活其 token。此实现保留 ADR-0002 的多来源差异，需要 KeyWeave golden 和后端真实检查，不能仅凭纯 matcher 测试宣称整条链与上游等价。

资源：即使 owner 已提前 up，其旧屏蔽快照仍计入引用预算，规则结束才回收。含 mandatory 的新 down 在状态变化之前检查保守的两阶段上界，稳定态引用不超过 16384；up 的临时作用域至多两层，加上 16384 owner 预算，峰值上界 49152 引用。预算不截断已有 up/Reset。展开预算包含各修饰作用域边沿；SourceToken 临时分配仍在旧 binding 的区间内。压力测试批量释放被屏蔽 owner 后保留规则，确认超限不改变既存状态，Reset 完成清理。

验证：原样保留上游 modifier-matching.jsonc；33 个标准匹配案例通过，1 个含 Fn 的输入案例明确跳过。10 个引擎生命周期测试覆盖左右组、optional、any、同 modifier 输出无多余边沿、不同来源、旧快照宏、设备拔除、lazy 指针刷新与资源耗尽；另有 compiler/CLI 黄金测试。mock 证据不代表 Linux/Windows 真实 modifier/LED/布局行为通过。
