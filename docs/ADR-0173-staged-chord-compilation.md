# ADR-0173：组合键编译与会话模式绑定

问题：分阶段会话已经支持组合键，但公共编译器无条件 KW-SIM-001，使 host/worker 启动与 reload 无法装载用户规则。

选择：新增 CompileMode::{Legacy, StagedKeyboard} 与 compile_with_mode；既有 compile 委托 Legacy。只有显式 StagedKeyboard 解除 Complex simultaneous 的 KW-SIM-001，使用同一解析/IR/后端能力/有损诊断检查，不靠修改原 JSON 或删除诊断。Simple 组合键、无支持字段/动作、设备能力、规则原子禁用、输出上限均不放宽。

Windows/Linux PreparedConfig 以其固定 ExecutionMode 选择编译模式，保留原 bytes 与 generation；随后仍经原生能力和该模式 controller 冷预检。reload 的 Target::WindowsStaged/LinuxStaged 已调用同一工厂，并在实际 actor apply 前核对模式，不能因准备成功自动确认 activation。默认 load_config/compile/CLI 与 Legacy 仍拒绝组合键。用户选择 staged-keyboard 后 host 与 worker 各自编译精确交付 bytes，不开放新捕获方式。

替代：全局解除编译门禁会让 Legacy 启动失败且模糊可执行标记；利用 capabilities.name 或 JSON 内自报模式无法证明实际执行器。显式参数由现有会话模式决定，启动与 reload 共用。

证据要求：默认/显式编译对比、完整 options/旧 Arc、非法规则和平台能力拒绝、精确 bytes/generation、真实 reload job 的 mode mismatch 与 accepted/rejected ack。Linux 离线检查和 Windows 原生/hardware 验证分别报告，无新依赖/FFI/unsafe；Windows 仍是合并流，不承诺设备身份。
