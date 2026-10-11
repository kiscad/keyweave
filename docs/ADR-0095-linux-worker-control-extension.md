# ADR-0095：Linux 活动 worker 控制扩展边界

日期：2026-10-11。状态：实际循环扩展入口与 reload Controller adapter 已实现；真实 main/host 尚未采用新入口。

## 顺序与独占

原 `run_with_poll_control` 增加默认无操作 after_poll/close，旧调用保持行为。活动迭代仍先接收 Stop/progress、检查 consumer、执行真实 Runtime.poll；只有 Runtime 仍 Active，才执行扩展，再由原 client.publish 回答实际 poll epoch。扩展不获得进度发布接口，也不能为后台编译/正文/metadata 构造 epoch。非 Active 阶段跳过扩展；扩展控制也不在 Recovering 时查询 guard/consumer/parent 活动。

WorkerExtension<S,B> 接收原独占 mutable Runtime 与 observation callback，用于一个 nonblocking 控制操作；要求 callback 前后包围 IO/激活。Linux `reload_control::Controller` 实现此 trait，将真实 ControlObservation 转成 ADR-0094 Evidence，不替换 actor、不创建第二个引擎，不复用输入/Stop/progress 流。

public `run_console_worker_with_extension` 只接受已通过 ConsoleBootstrap 的原 runtime/provider/ready/clock/guard，检查 isolated group 与 progress FD NONBLOCK。ExtendedControl 先执行原 ConsoleControl 刷新，观察现有 lease.valid_until，不扩大 console TTL 或自动重 arm；随后在每个扩展观察前检查同一 main 的 parent-death/group 保护，使用同一 CLOCK_MONOTONIC/连续性 clock，并以本次实际 epoch 接收 Stop/progress。

扩展操作中已收到 Stop 时立即锁存，不再调用第二次 clock 或续租，外层即使扩展返回错误也优先 pause，以 Runtime 最后接受的迭代时间恢复。扩展错误作为独立 WorkerError::Extension 保存；原失败路径 ControlFault、capture-first 关闭和最多原恢复预算继续生效。完成停止/错误恢复后调用 extension.close，再取得 termination、drop Runtime、最后 drop console provider，不能以 extension close 证明全局恢复。

## 独立父端活动预算

旧 worker 没有可直接提供给 reload 的父端时间 lease；新增扩展模式建立独立 2 秒活动预算。它从首个有效 Active console 检查建立，初始 last-request=0；不是 bootstrap/host watchdog 期限的继承，也不修改 host 的 startup/lease 策略。模式要求受信 host 持续在 2 秒以内交付新 progress 请求；更慢的自定义 host 会 fail closed，未来 host 启用此模式必须验证策略兼容。

只有 ProgressClient 严格接受的新连续 request id 能续期。shared client 新 last_request_id getter 只返回已有协议状态，Linux adapter 不改变 id 验证规则；Stop 单独处理，不续期。重复 id、准备完成、reload IO、配置确认、实际 input poll 自身都不续期。新的活动期限按接收控制 IO **之前**的 clock 锚定；控制 IO 后必须仍在旧期限内，不能用新请求跨越旧过期。倒退、gap/replay、加法 overflow 或严格 at>=until 失败；before_poll 也检查旧期限，阻止失效后的下一次输入 dispatch。

observations 另要求旧 console grant 尚未过期，不在扩展内部查询/更新 provider。wait deadline 取原 console 刷新、父端活动期限、每次 observation+5ms housekeeping 最早值，后台准备/Ready/部分 ack 不会无限等待下一输入。所有额外进度读取有界且 nonblocking；ADR-0094 的“最多一次 transport IO”指 reload IO，不包括最多两次 observation 中的独立 progress 接收及只读 guard/clock metadata。

## 生命周期和后续

扩展按 `&mut E` 借用，返回后原 owner 仍归调用方持有，能用 Controller.reap_closed 回收同一个后台任务；close 不等待编译，不重新启动任务。真实 main 必须保持这个 owner 直到恢复/收尾或实际退出，不把 Drop/detach 当退出证明。trait 是受信进程内接口，不能强制任意恶意实现调用 observation 或避免阻塞，原生 Controller adapter 才落实前后证据检查。

本轮不修改真实 worker main 或 host spawn。下一步创建带 reload 的 guarded startup→bootstrap→新循环→terminal 收尾组合，并让 host 在策略兼容时采用 ADR-0093 新模式。None 永久无设备 invariant 保持，自动测试不得调用成功 console bootstrap/真实抓取。之后仍需 watcher/reload CLI、Windows CaptureSession adapter、桌面 provider、完整规则/GUI/driver 与硬件 gate。

## 验证

Linux Rust 1.97.1 locked/offline workspace/all-targets 920 tests、Clippy -D warnings、fmt、60 项固定来源校验通过。新增 3 项纯活动预算测试覆盖只按连续新请求续租、重复不续/严格过期、倒退/replay/gap/overflow、PRE-IO 锚定与旧过期间不得续期。新增 1 项共享 worker-loop mock Runtime 测试含 Stop/fault 两分支：真实模拟输入 poll→激活新配置→旧持键 up 恢复，trace 明确 InputClose 在 recovery submit 前、OutputClose 在 ExtensionClose 前、provider 最后关闭，错误不抹去且不再等待。

实际 native ExtendedControl guard/provider/Stop callback 和 5ms wait 未进行真实 console/hardware验收；Controller 的 native Runtime trait adapter 已编译，mock trace 使用受信测试 PollControl。未新增 unsafe/FFI/dependency，C ABI oracle 未重跑；既有 no-input actual-main harness 随 workspace 通过。Windows 新 getter/前序 shared/native 检查仍待补，新增 Linux-only 测试不提供 Windows 实测证据。见 [验证记录](validation/linux-2026-10-11-worker-extension.json)。完整目标继续未完成。
