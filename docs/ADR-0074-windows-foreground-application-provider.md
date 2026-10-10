# ADR-0074：Windows 前台应用路径 provider

日期：2026-10-10。状态：代码已连接活动会话，Linux mock 验证通过；Windows 原生编译与桌面验收待完成。

## 行为

CaptureSession 活动控制循环在访问检查后、callback 外调用 application sampler，并经 ADR-0073 的 SessionActor 入口交付上下文。首次活动 poll 采样，之后最早每 100ms 刷新；已有消息等待最长 20ms，空闲时仍有控制机会。更新不单独增加 progress_epoch，不能用 context revision 给监督租约续命。

采样依次读取 foreground HWND/thread/PID，以 PROCESS_QUERY_LIMITED_INFORMATION 打开非继承进程句柄，查询 Win32 可执行路径，再读取 foreground tuple。无窗口、标识无效、打开/查询失败或 tuple 改变时返回 Unknown，不复用旧路径。

QueryFullProcessImageNameW 使用固定 4097 个 UTF-16 单元，严格验证成功长度、终止符、内嵌 NUL、有效 UTF-16 和 4096 bytes 属性上限。失败或超长不截断、不做有损转换；不读取标题、账号、命令行或键盘状态。原生 Unicode 路径可保留，但当前 Engine 对非 ASCII 条件属性仍返回 Unknown，未扩展正则兼容范围。

依据：[Microsoft QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew) 明确 query 权限、flags=0 的 Win32 路径、以字符计的输入容量，以及成功长度不含终止 NUL。本次在 SDK C oracle 增加其 WINAPI 原型检查；尚未执行更新后的 oracle。

## 时间与生命周期

使用同一 SessionClock，observed_at 在查询前采样，有效期严格为 observed_at+500ms；查询后时间不能延长这份 metadata 的有效期。迟到结果发布未知，下一查询机会按完成时间+100ms 安排。再次校验 access lease、紧急组合和已有 Hook fault，再交付；时间/revision 耗尽不 wrap，而停止映射。

OS 查询期间不借用 actor、lease 或 emergency。同步送达的 callback 仍能使用执行者；如果 callback 已选定 fault，不发布上下文，保留该主因。先到期任务使用旧快照，已有 binding 仍由原 up 释放。

application provider 是 CaptureSession 内当前唯一 context writer，revision 连续递增，devices/input_source 为未知，库存范围为 Partial；不声称完整库存，不编造 bundle ID。后续输入源或设备 provider 接入前必须增加统一字段合并，不能各自发送覆盖其他字段的完整快照。

进程句柄只有一个 owner，查询结束明确关闭。CloseHandle 失败不被未知 metadata 或主 Hook fault 覆盖：保存 `ShutdownReport.application_provider`，停止捕获并走既有 unhook-first 恢复；不把 metadata 关闭错误当作桌面访问撤销。关闭仅一次尝试，失败表示未确认释放，可能直到进程退出才回收；不能称完整资源清理成功。Worker main 将该独立错误纳入终止 fault 位，即使输出恢复成功也不是 clean exit。

## 限制与验证

前后 tuple 比较不能证明整个间隔中前台没切换，更不能排除 HWND/PID 重用的 ABA。采样不是图像认证、每事件原子上下文或输出权限证明。100ms/500ms 是刷新策略，OS metadata 调用不是非阻塞保证；实际事件循环监督继续独立运行。持键切应用、权限切换、睡眠/RDP/安全桌面必须实测。

新增 4 项原生 API 替换测试：Unicode 路径、不编造 bundle、失败/前台变更、严格 buffer 验证及关闭失败。新增 3 项会话集成测试：真实 poll 路径发布和查询失败清除、关闭错误独立保留与恢复、限频/查询前有效期/借用安全的同步 callback。这里的 Hook 与 API 均为 mock，不安装真实捕获。

固定 Rust 1.97.1：Linux workspace 786 tests、all-targets Clippy、fmt、60 个来源 hash 通过。初轮新增 fixture generation 重复导致拒绝，改为 generation 2 后重跑完整测试，不放宽生产约束。详见 [验证记录](validation/windows-2026-10-10-application-provider.json)。

本轮 Windows 原生编译/SDK/桌面测试未执行：前一轮自动审批因额度限制无法完成，未绕过或使用旧快照冒充验证。代码和 FFI 均须补跑双架构原生验证；完整项目仍未完成。
