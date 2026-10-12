# ADR-0139：保存 batch 键来源的 Terminal 投影

问题：Simple scope 后续需要按保存来源临时隐藏普通键/标准 modifier，而不能按当前配置重选或按键码释放其他来源。ADR-0138 已保留完整原计划。

选择：TerminalActor.begin_key_projection 捕获仍 Reserved/Live 的原 slot key/source 及 snapshot 临时 owner，返回不透明 KeyProjectionId；Down/Up 控制 marker 进入原 Output FIFO，实际交付才修改 hidden 引用。非 modifier 立即同步可见性，标准 modifier 保留 lazy 语义。嵌套引用仅最后 close 恢复，同键其他来源不受影响；被释放/取消的来源不生成假 Down。booked suppression reference 在准入时受16384限制，即使 marker 尚未交付。projection数量有界，ID跨Reset单调。

variable slot 尚无生成/实际两侧的暂存协议，明确冷拒绝。这只实现 Terminal 键来源投影，不解除 Simple consumed scope gate，不代表 Output 生成视图、Complex 后代继承或完整 Simple scope 已接通。控制器/native未切换。

begin/close准入在任何引用/ID/时钟变更前验证目标、预算与FIFO容量；queue full允许同token重试。device/cancel/Reset丢marker与引用，恢复只针对实际emitted来源。Projection不是OS接受或输入鉴权。

验证：8个公开Terminal集成回放覆盖旧配置/嵌套、同键其他来源、原Up后不恢复、FIFO future array、modifier lazy、device/Reset、variable冷拒绝和snapshot临时来源；3个私有容量测试覆盖16384 refs预book、full FIFO begin零ID/ref/clock副作用、full FIFO close保同token并在释放一个slot后重试。Linux workspace1271 cases、Clippy -D warnings、fmt、source60+fixture copies通过。首次脚本因observe tail定位停止，使用child module完成实现后无unused；没有native/硬件/Windows验证。见 [验证记录](validation/linux-2026-10-12-terminal-key-projections.json)。
