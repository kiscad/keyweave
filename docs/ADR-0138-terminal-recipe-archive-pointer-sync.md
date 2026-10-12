# ADR-0138：完整 Terminal 原计划与指针同步边界

问题：Terminal 准入将初始 events 从原计划取走，已接受 batch 仅保留 saved Up，无法从实际 owner 追溯全部已过滤的原 targets/slot/旧快照。投影后续需保存的目标而不能重选当前配置。另外 mixed pointer 实际交付使用无条件 sync_modifiers，绕过原 Engine 的重复键/按钮保护。

选择：时间线工作从原计划 events 克隆，batch 保存完整不可变计划。saved_plan 只读返回当前尚未完成 batch 的原计划；原 earliest 不代表 FIFO 位移后的交付时间或 OS 接受时间。计划的 targets/slots/旧 Arc 固定，不因热更新、poll 或保存 Up 改写；实际完成/控制取消后不再返回。记录仍受既有 batch/slot/event展开容量限制。

pointer 实际交付改用与 legacy Engine 相同的 sync_pointer_modifiers：正在重复或有按钮按住时不主动刷新 lazy modifier；实际新按键照常同步。仍返回捕获的 pointer receipt，不改变 mixed FIFO/设备/Reset 协议。

该原计划 API 是投影所需的元数据基础；尚未实现 Simple consumed scope 到普通键/变量后代的释放和恢复，原 gate 保留。不把读取原计划当实际 Live/可见 owner/原生接受证明。

上游证据：固定源码 post_event_to_virtual_devices/post_event_to_virtual_devices.hpp 的118和394行检查 !keyboard_repeat_detector.is_repeating()、pressed_buttons.empty() 及 !mouse_key_handler.active()。本改动复用已实现的 Engine repeat/button条件；mouse_key 仍不支持，未宣称覆盖该状态。来源文件hash见验证记录，未修改上游或执行mac tests。

验证：两个物理回放测试覆盖repeat/按钮持有→pointer不flush→释放后idle pointer flush，以及真实新Down仍先flush再keyDown；一个Terminal回放覆盖原计划targets/slots/旧Arc在FIFO shift、实际Down、reload、saved Up过程中保留，实际完成后移除。Linux workspace1260 cases / Clippy -D warnings / fmt / source60+fixtures通过。见 [验证记录](validation/linux-2026-10-12-terminal-archive-pointer-sync.json)。
