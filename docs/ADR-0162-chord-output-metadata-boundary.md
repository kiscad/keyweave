# ADR-0162：完整组合快照的 Output 元数据边界

Output/Terminal 只执行已经选定且联合准入的 scoped plans，不应因为保存的原 Arc 还包含 simultaneous 定义而拒绝计划，亦不能删掉定义或换成伪造的配置副本。本层校验完整定义的资源/类型和变量元数据，保留旧 Arc；Legacy Engine、Simple/Complex 输入 actor、Physical controller 与原生准备继续严格拒绝尚未接入的组执行。

private Engine 的 allow_action_timing=true 仅用于 Terminal 输出模型；这一路径和 Output 目的 Generated queue 用明确 metadata 校验，根输入执行路径不因此获得组合匹配能力。普通 selector 仍跳过组合规则，配置仍 KW-SIM-001 disabled。

测试将 ChordArbiter 的实际 Matched selection/成员传入共享 Down+Output 联合 guard，在真实 Output actor 上执行 main 数组，并由 ActiveChord 的 any/all 事实驱动保存 input_up；全成员原 Up 且实际输出完成后才解除共享 handle/回收参与者。独立代码/测试验证这条接口路径，不宣称完整 Complex actor 驱动或 OS 接受。

实际组 actor 仍须统一管理 matcher、输入表、空 main、branches/投影/after-group-up、条件身份、设备/reset、lease/replay/repeat；公共 runtime gate 不撤。无 native 捕获/注入，Windows 本机补验继续独立待完成。
