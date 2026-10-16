# ADR-0236：Settings Windows 状态/停止与旧请求结果隔离

Settings 控制页按本机平台接 Linux socket 或 Windows endpoint。Windows 复用 Client machine/pinned transport，单帧至多一次 transport 方法，绝对1s事务与100ms额外关闭（总体不越1.1s），不在 UI 写 sleep/等待循环、启动 host/worker 或发送输入。冷准备及单次 SDK 调用仍为同步，不声称硬实时或无延迟；native !Send owner 留在 UI 同线程。

公共 WindowsEndpoint 解析/格式类型移到 progress DTO 模块，host 保留 re-export；UI 与 CLI 使用同一有界格式、PID/完整 FILETIME 校验，不复制 parser 或依赖 host runner。正常 UI 粘贴完整 cold host descriptor，不要求逐字段输入 PID/FILETIME。没有自动发现/启动/Resume/Apply。

pending request 保留 endpoint 和 display 资格。切换 endpoint/Disconnect observer 清除 Stop 资格与 confirmation，sticky 禁止旧 pending 结果再次成为可停止的当前观察；请求仍驱动到有界结束，不承诺取消已提交 Stop。失败清除可操作资格但保留带来源/age 的最后观察。Stop 仍需当前 endpoint、fresh<2s、原 session 和二次确认，关闭 GUI 不主动关闭 host。

可测试的泛型 Windows GUI request driver 使用真实 Client machine 和 scripted transport，验证 pending、一次方法/帧、匹配回复、清理未知/错误与绝对期限；不是实际 Windows pipe/SDK/GUI 显示验收。Windows 专属 factory/wiring 未原生编译运行，既有审批额度拒绝不重试。Linux 原生 ignored gate 不因本默认回归冒充执行。GUI Start/Resume/Apply/发现、桌面与硬件验收及完整迁移继续。

验证：新增5默认tests（4泛型 GUI Windows driver/真实 Client codec＋1观察失效），全量Linux1998通过，8 native默认ignored；格式/Clippy/源码62/Linux ABI/X11 SDK/regex oracle通过。独立获准IPC环境实际运行既有8项Unix socket/CLI/owned无输入host/GUIclient全部通过；不代表桌面点击、WindowsSDK或输入恢复。文件创建过程曾在tests模块尚未写入时运行fmt并报告缺文件，补齐后最终检查全部通过。见 [验证记录](validation/linux-2026-10-16-windows-settings-control.json)。

后续原生 gate：Windows x64/x86 编译 UI/host/native_link，真实无输入 host descriptor→GUI Query→fresh nonce两确认Stop→原owned child actual exit，覆盖切端点/断开/旧pending/EOF/清理超时；再验证实际窗口/焦点/字体/a11y。不得把 Linux generic driver/API mock 或Unix socket通过称为这些Windows验收完成。
