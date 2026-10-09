# ADR-0039：Windows 映射前紧急组合

问题：规则可能吞掉或改写退出组合；只在下一个按键处理时检查，无法在持续按住期间退出。Windows Hook 不提供可信每事件设备身份。

选择：固定 Ctrl+Alt+Shift+F12，左右任一、连续 >=1s，按 CanonicalScan 解析后的原物理 HID 身份检测。injected 在 dispatcher 中先 bypass，passive startup 不建立 emergency 状态。Active handler 和 control 都用同一 delivery time，先 advance emergency，再 observe 物理边沿/交付 actor。重复不建立新按住、不刷新期限；任一组释放撤销，左右交叠保留原期限。control 等待取 emergency 与 actor deadline 的较小值；紧急到期先于同刻普通 up、输出或 engine timer。

触发返回专用 HookFault::Emergency，native 立即禁用 capture；CaptureSession 转为 Emergency step，先 close Hook，再按 accepted ledger 至多 8 次恢复。探测错误单独保留，不当作成功紧急触发。主检测/actor 错误、卸载和恢复结果分别可检查，不丢弃未知 receipt 的 quarantine。不在 callback 中卸载或恢复输出。

替代：按映射输出或 injected VK 检测可能被配置绕过；RegisterHotKey 不表达连续按住与原始左右身份，而且没有排在引擎前的同刻优先级；后台 sleep 会改变单执行者时序。

边界：Windows 是合并 stream，允许不同键盘共同组成该组合；不声称 Linux 的同一物理 device 保证。Hook 被移除/线程卡死/安全桌面不受此检测保护，独立进度与 session 监督仍必做。本轮只替换 API 测试，不证明 Windows 运行行为，repeat、碰撞、可执行 host、SDK/硬件验收继续保留。
