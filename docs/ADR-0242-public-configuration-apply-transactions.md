# ADR-0242：公开配置 Apply 事务与确认边界

问题：设置界面只有 Status/Stop，现有私有 reload 通道不能直接暴露给 GUI；队列接收、编译成功、实际激活及确认必须分开。配置 watcher 目前独占 reload source/outcome，不能由另一入口偷取确认。

选择：在 keyweave-progress 新增独立公开 KWAQ/KWAR v1 48-byte 请求/回复 DTO 和平台无关单事务协调器。请求携带公开 session、非零请求 ID、base/target（严格 base+1）和1..8MiB字节长度，不含私有 worker run ID。回复只能根据对应 backend ticket 的实际完成事实标记 Committed；Rejected/Cancelled 必须仍为 base。Busy/Unavailable/会话或代号过期拒绝在提交前完成。生成差异或错误 ticket 不得变为成功。

事务持有不可变完整 bytes，一次性转交给独占 backend；每 step 至多一次 backend 操作。所有时间注入，绝对期限不因 pending 延长，调用前后检查时间。后端调用前先进入不可重试状态；panic、异常结果、超时或确认丢失都保留 may_have_applied，不自动重放、不回滚。提交前明确拒绝或经核验的实际 Rejected/Cancelled 才能证明本事务没有应用。该事实仅关于本事务，不表示其他来源没有改配置。

backend 的 ticket 必须绑定实际独占 reload 请求与准确 bytes/base/target；这是宿主 adapter 的可信约束，纯状态机不能替代进程认证/worker ACK。完成不是进度 receipt、输入恢复或进程退出证明。公开帧与现有 Status/Stop 和私有 reload 完全独立，不修改它们的 ABI。

本轮实现可执行的 codec/协调器与客户端回复校验、确定性故障测试及真实冷编译→Engine.activate 组合测试。尚不接 OS transport/host/GUI：后续必须增加有界传输、同用户认证、watcher/GUI 单一 reload source 仲裁和实际 native ACK adapter，再开放 Apply 按钮。Linux/Windows 都使用同一公开协议；不得把这些离线检查称为 GUI Apply 已完成。

替代：直接复用私有 reload ID 会泄露内部身份且绕过宿主监督；通过保存文件冒充 Apply 不具备请求级确认；将 admission 当成功会在编译拒绝/丢 ACK 时误报。均不采用。

验证：新增10项测试全部通过，Linux workspace 2031项通过/8项既有 native IPC 默认忽略；格式、Clippy、62上游源码、regex oracle、Linux ABI、X11 SDK 共七项检查通过。最终收紧代号校验和 Rejected/Cancelled 标志后重跑全套检查。没有实际输入或 Windows 原生调用。见 [验证记录](validation/linux-2026-10-16-public-apply.json)；后续宿主仲裁、传输与 GUI 的明确接线要求见 [协议文档](SESSION_APPLY.zh-CN.md)。
