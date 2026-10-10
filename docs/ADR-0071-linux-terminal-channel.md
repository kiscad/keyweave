# ADR-0071：Linux 独立终止报告交付

共享 progress 帧必须保持固定格式和因果租约，协作停止后不能读取旧 reply 续租。新增显式 reporting 启动模式，把 stdout 替换为第三条独立非阻塞匿名管道；旧两管道模式不变。Worker ReportingStartup 在真实 main、打开设备之前核验父保护、report模式和三条管道方向/不同身份，沿用一次性claim；只复制CLOEXEC协议FD，不接管或关闭原标准流。stdin/stderr/stdout在report模式均为私有二进制端点，不允许stdio日志。结构核验不是跨用户认证。

KWLT/v1固定32byte：magic、version、reason枚举、10bit事实mask、正u64 run ID，其余reserved0。编码/解码严格拒绝未知reason/reserved/run与自相矛盾的恢复确认；不含按键/path/设备身份。TerminalWriter只prepare一次，单次flush最多一次Write，部分返回/WouldBlock保留原frame，无自动重新编码/重复发送；fatal错误或非法count永久禁止重试，不能猜测错误后的接受量。发送完成不是实际退出、清理或全局恢复证明；真实exe仍须显式有限发送并实际退出，未成功不能报告clean。

Supervisor.spawn_reporting独占第三条reader，仅在实际try_wait Exited后poll_terminal；每次至多一次非阻塞Read，最多32+1 bytes，完整frame且EOF才Complete，少字节EOF为Missing、额外字节/坏frame为Invalid，尚未EOF/Pending不证明报告完整。无report模式为Unsupported，未实际退出为NotExited。收到报告不改Supervisor reason/exitcode/progress epoch/租约。host仍需为报告观察设有限deadline，拒绝缺失/无效/未确认，不把任何synthetic事实当实际键盘证据。

验证：codec mask/ID/格式与矛盾拒绝、partial/WouldBlock/重复prepare/EOF/超长、旧模式与preExit读取拒绝、真实main ReportingStartup+第三管道往返/一次claim（无设备）。原生stdout仍在child生命周期中保留，只有实际exit才证明其关闭；丢弃复制writer不是EOF证明。Linux实际capture bootstrap/consumer-ready/完整host仍待实现。

最终验证：Linux764 tests及扩展真实main无输入harness，Windows x64/x86各724 tests、Clippy/fmt/既有SDK通过，8 portable tests及main第三pipe synthetic cases。[报告](validation/windows-2026-10-10-linux-terminal-channel.json)。KWLT交付终止事实，非详细WorkerError序列化；原outcome/exitcode独立。初module缺文件和bit整数推断失败已修正并记录，未放宽协议。真实capture exe/host、有限发送/报告观察及全部完整目标继续。
