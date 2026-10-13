# ADR-0172：现有分阶段键盘会话接入组合键

问题：Physical 组合键模型已经验证来源预算和生命周期，但 Linux Runtime 与 Windows SessionActor 的 StagedKeyboard 工厂仍构造严格 controller，无法通过实际输出泵验证组合键和 repeat。

选择：StagedKeyboard 明确选择 PhysicalStageController.new_with_chord_model；Linux 的模式冷校验沿同一入口。沿用已有时钟、64-work、输出准入/accepted ledger、恢复和设备身份，不另造并行会话或定时器序号。Legacy 构造不改变。编译器 KW-SIM-001 仍阻止用户配置的原生激活，待完整会话和配置能力路径验收再解除；这里的手工 snapshot 测试不代表已验证硬件。

所有 main/普通分支/全组 after-up 输出仍通过 Mapping.actions 原生能力检查。Windows 合并输入流仍拒绝 Device 条件，不猜 RawInput 关联；repeat 只沿 actual saved leaf 且 desired/submitted 已接受。Linux 保持 Kernel repeat，不把 EV_KEY=2 作为新 root 或软件重发。停止仍遵守 Windows unhook-first 和 Linux input-close-first；待窗口取消不得编造未发送 Down 的 Up。

替代：增加第三种模型模式会让 host/worker/reload 的模式证据分裂；复制会话会绕过 accepted ledger。使用同一 StagedKeyboard 引擎接线并保留编译门禁，更便于继续验收同一实际路径。无新依赖/unsafe/FFI。Windows 原生编译、真实捕获及平台验收仍独立待验证。
