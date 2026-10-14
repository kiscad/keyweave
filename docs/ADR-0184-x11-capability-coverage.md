# ADR-0184：X11 元数据能力覆盖检查

探针结果不是 consumer grant。新增独立、无副作用的能力检查，输入显式 X11 keycode/button index 需求和原目标，重新核验目标及报告结构，返回匹配的 keyboard/pointer slave ID 与 master attachment。每一种设备的完整需求必须由同一个 slave 覆盖，不能拼接多个部分设备；空需求、无匹配、歧义均拒绝。

需求限定核心 X11 keycode 8..255、button index 1..256，数量有界、排序和唯一性严格。这些是 X11 编号，不接受 HID usage 或 evdev code 的隐式转换；映射编译及 driver-specific button mapping 必须后续独立核验。仅按钮能力不证明相对移动/滚轮能力，当前协议未包含 valuator class，因此不得用于 relative capture 授权。

检查所得结构刻意称 metadata coverage，无时效、会话或 output-node ownership 的授权能力。报告字段可被调用者修改，检查入口必须重新验证，不信任先前 decode。原生 provider 仍需 owned 输出节点、会话前后一致性、inhibited 状态和固定时限；SDK 和真实服务器验证仍待完成。
