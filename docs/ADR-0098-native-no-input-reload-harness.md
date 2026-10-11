# ADR-0098：真实父子进程的无输入 reload 组合验证

日期：2026-10-11。状态：真实 NativeSession/端点/后台编译/纯 Engine 组合已验证；console 硬件与用户 watcher/reload 命令仍未完成。

## 原生组合

新增 keyweave-host 的 linux_reload actual-main harness（harness=false）。父进程在真实 main 安装 HostSignals、恢复 child mask hook，创建 sealed 配置，以 Supervisor reload/isolated 模式启动自身的受信子模式。Session/NativeSession 取走原 supervisor 的绑定 run 端点，使用原 MonotonicClock、实际 progress watchdog、signalfd 与实际 child Exit/terminal 观察，不替换生产 parent lane。

子模式验证 main parent-death/group、sealed exact bytes、双 reload selector/单次 claim。原 stdio FD 继续保留；一个唯一 shared ProgressClient 的 CLOEXEC copies 处理独立 progress/Stop，reload Controller 持有专用端点。实际 Linux PreparedConfig、后台线程、Engine.activate 与固定协议确认仍走生产代码。只新增已有 workspace-local Engine 的 dev dependency（lock 一行），无新生产行为、FFI/unsafe/外部包。

## 合成证据边界

子进程没有 Runtime、ConsoleProvider、CaptureGroup、evdev/uinput、Hook/SendInput 或输出 ledger。输入是固定纯 Engine Down/Repeat/Up；每个真正收到的 progress Challenge 触发一次合成 Engine iteration，fixture 对外发的 Active/epoch 是**合成进度**，不是设备 poll 证据。合成 session deadline 与父请求期限也不证明真实 consumer/readiness。这个 harness 用它们测试真实 NativeSession 的 Active admission/监督协议配合，不将其称为成功 console 捕获。

子进程不发送任何 KWLT recovery frame。父端明确要求 terminal=Missing、recovery_confirmed=false、successful=false，即使 actual exit=0；不能把纯 Engine up、正常退出或闭管道当全局键盘恢复。

## 两个实际子进程场景

Accepted 场景在实际 Active receipt 之前检查 request_reload 返回 WouldBlock；Active 后空候选为 InvalidInput。发送含 96KiB comment padding 的合法候选，验证真实分段 pipe 传输/后台编译/实际 Engine generation=2/Accepted，队列在 pending 和未取 completion 时均 Busy。取 Accepted 后再送无效正文，Rejected 保留 observed generation=2。期间原 progress 同时持续推进，parent fault 保持 None。

随后只向 **测试自身父 PID** 发 SIGINT，signalfd 的原 NativeSession 控制路径协作停止同一个 child。子进程原持键来自 generation=1，repeat/up 仍输出旧 b；新 down 输出 c，证明实际切换与旧 binding 保留。父端 actual code0、Cooperative、Interrupt、signal close无错、epoch>0、Missing terminal；closed lane拒绝新请求，重复观察返回同一缓存 Report。

Lost-ack 场景在子 actor 已实际 activate→Acknowledging、代号=2且旧 binding=1 时，不执行任何 ack write，关闭并回收原 cold owner后正常实际退出。父端 observed generation保持1、completion=None；actual code0证明子模式中激活断言完成，但不能让父端推断成功或自动重试。原 selected FD 保留至真实退出，不用人为关闭原 stdio伪造提早进程退出。

全部等待只驱动原 transaction/原 child。5s交易观测和10s最终观测不重启任何任务；观测超时会失败而不是重开窗口。子线程只冷编译，main parent-death保护的创建线程始终存活。

## 验证结果与剩余

Linux Rust1.97.1 workspace/all-targets locked/offline 的931项 libtest、独立 linux_reload 两场景 actual-main harness、Clippy -D warnings、fmt、60项固定来源校验通过。harness 不计入931项 libtest；日志明确记录独立结果。未新增 C ABI 边界，oracle 未重跑。Windows该 harness为无操作 cfg stub，不能以此背书原生 Windows；dev依赖和前序 shared/native 检查仍待补执行。

本轮增强原生组合证据，没有修改生产运行逻辑。下一步接 host 的有界文件监听与用户 reload 操作，再做专用真实 console/input/权限/恢复与性能验收。Windows adapter、完整高级规则、图形桌面/GUI/driver等原始目标仍未完成。见 [验证记录](validation/linux-2026-10-11-native-reload.json)。
