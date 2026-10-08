# ADR-0007：输出修饰动作与设备归零

状态：标准 HID modifier scopes 已实施；2026-10-08。

来源：固定上游 to_event_definition.hpp 的 make_modifier_events，basic/event_sender.hpp 的主动作与修饰释放，shared_event_sender.hpp 的 post_tap，event_queue/utility.hpp 的设备所有物理键已释放事件，以及 post_event_to_virtual_devices.hpp 的 modifier dispatch。抽象输出组使用第一侧；modifier 集合按上游枚举次序，而不是 JSON 数组顺序。

选择：Action 保存有序输出 modifier 键。修饰作用域进入下一事件阶段，可触发后续阶段规则；未被映射的 scope modifier 保留 lazy down/up 标记，匹配后的规则生成自己的动作策略，不盲目继承输入 lazy。普通主键 down 提交修饰，然后 scope modifiers 逻辑 up（lazy），主键可保持。最后主键是 modifier 时，scope modifiers 保持到原输入 up，即使主键 repeat:false 已完成脉冲。extra/up 动作完整轻触，scope modifiers 的 down/up 都是 lazy。

输出按 owner 计数；scope modifier 与同名物理 modifier 不混用 source。逻辑 lazy up 不立即发 OS up，下一 ordinary down、明确 modifier 事件、允许的 pointer refresh 或设备归零提交状态。按来源的 mandatory suppression 与这些 scope owner 相互独立。

设备归零：纯引擎已追踪每个成功捕获的物理 down binding；已知 up 结束后该设备没有其他 binding 时，提交修饰状态，对应上游设备 keys/buttons released 事件。未知 up 不凭空推断归零；拔除结束所有该设备 binding 后也刷新。后端仍须处理抓取起点非 neutral、SYN_DROPPED、未经过引擎的输入与 submitted 故障，不能把这里的推导当成完整 OS 状态重建。

轻触历史 modifier restorer 也使用 lazy 逻辑变化；设备归零负责最终提交，其他设备/同设备仍 held 时保持延迟。caps_lock/Fn modifier flag 暂拒绝；to.modifiers 的 any 按上游没有具体 modifier flags，因此明确接受为无动作。hold_down/显式 lazy/halt 等继续拒绝。预算按全部生成 modifier/key 的两阶段展开预留，不能只数主键。

vk_none 是可刷新 lazy modifier 的无键 down，不创建 owner；上游 vk_none_1 原始用例以它依次提交 Shift、Command、Option。该用例的五个报告边沿一致，最后仍 held Option，独立 Reset 清理。另一个原始 standard branch 用例的十四个报告边沿一致；完整原规则文件包含 Fn 分支，本次只编译该用例实际覆盖的 standard branch，不宣称完整源规则集兼容。Mach 时间不转换，case 使用注入顺序比较边沿。十个生命周期、三个配置与一个 CLI 新测试覆盖跨阶段匹配、提前释放、多来源、repeat:false、热更新、预算与 neutral；实际 OS/驱动行为待验收。
