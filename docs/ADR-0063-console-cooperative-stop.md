# ADR-0063：host 控制台中断只锁存停止，由独立主循环收尾

Windows控制台handler在独立系统线程执行，Ctrl+C/Break可由返回TRUE的handler处理；关闭/注销/关机事件有不同终止时限，不能靠立即返回TRUE保证恢复。设计依据：[SetConsoleCtrlHandler](https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler)、[HandlerRoutine](https://learn.microsoft.com/en-us/windows/console/handlerroutine)。新增ConsoleControl：process-lifetime单次claim、callback外注册/注销、!Send/!Sync同线程owner；handler只对0/1做static AtomicBool Release store并TRUE，对其他事件FALSE，不做分配/IO/锁/borrow/probe/恢复或等待。

独立static atomics一直有效，callback不持有stack/Arc/raw owner指针；晚到/inflight handler不触碰已drop对象。claim不重置，注册失败/注销失败/成功close均不得同process重装。键盘callback拒绝不消费claim。close失败保留registered状态可显式重试，Drop仅一次fallback，不作成功证明。requested是纯atomic元数据、事件锁存不清。GetConsoleProcessList单scalar检查：明确无console（error6）可返回不可用owner，保留stdin控制；其他错误不降级为可用。生命周期内不得Alloc/Attach/FreeConsole重置handler表。

main在冷配置编译完成后、artifact/worker创建之前注册，避免阻塞配置读取时捕获Ctrl+C而无法中断；早期默认退出没有active child。注册后输入线程与console原子标记合并，run_with_console在preRNG/preJob与每poll读标记，使用原取消/协作Stop/期限/actualExit/资源报告，不造心跳、不在handler调用Supervisor。报告输出前明确注销；主错误与注销错误独立保存，注销失败host非零退出。关闭console/注销/关机仍走默认处理/parentJob保护，无已知键恢复承诺。

已有worker创建flags=EXTENDED_STARTUPINFO_PRESENT|CREATE_SUSPENDED|DETACHED_PROCESS，不继承host console，避免console广播直接终止worker越过协作协议；本轮不改变该flags/继承契约。[创建标志依据](https://learn.microsoft.com/en-us/windows/win32/procthread/process-creation-flags)。SDK oracle增加handler原型/0、1、2、5、6常量与DETACHED_PROCESS值。

无输入诊断在无现有console的自有helper中新建私有console、立即隐藏自己的窗口，安装同一ConsoleControl，延后生成只发往该私有console的0/1事件；body执行真实host run_with_console→ownedJob→独立协议fixture。event producer在console释放前join，注册/事件生成/观察/注销/释放错误独立报告，不对用户console broadcast，不向键盘注入。拒绝复用现有console，单process一次，不Free用户console。实际child fixture已detached，不受广播，只从Stop帧退出；epochs仍合成，不能证明CaptureSession恢复。

验收覆盖mock注册/claim/callback/0与1/其他事件、不清锁存与晚到callback、无console/错误准入、注销失败retry/Drop及并发atomic可见性；真实双arch私有console驱动host与协议fixture，旧stdin/超时/取消/invalidPE回归。真实物理Ctrl+C/Terminal/RDP/关闭窗口/正常键盘仍需验收；热更新/详细诊断/可信打包、Linux/D05/GUI/Rustdriver/companion及原目标继续。


启用补充：有console时，先对本process调用SetConsoleCtrlHandler(NULL,FALSE)，确保继承的ignore标志不会吞掉Ctrl+C；失败永久拒绝启动，单独保留enable错误。close移除本handler，不声称复原未知的原ignore属性；不会更改其他console process的handler或标志。callback是safe extern-system leaf（没有需要调用者保证的内存参数），传至Win32 unsafe callback指针类型按同ABI转换；测试无需多余unsafe块。

验证与诊断启动修正（2026-10-10）：初次原生运行发现 .NET CreateNoWindow helper 仍附有隐藏控制台，准入明确拒绝，body/worker 未启动，x86 原生阶段未执行。保留该失败，不允许释放或复用既有控制台；最终 helper 先以 DETACHED_PROCESS 自启动自有子进程，再在子进程内创建私有控制台。外层只使用所持 Child handle 观察/终止该子进程，10秒请求终止、12秒观察上限，不按任意PID结束进程；未确认退出不报告成功。

ConsoleProbe 的 delivery/unregister/release 独立报告只覆盖成功进入 body 后的路径；创建/安装失败在进入 body 前使用有限 Drop 回退，不能据此证明所有清理错误均已报告。真实双架构共8个 console case 与10个原 host case 最终通过；Linux702、Windows各673 tests、Clippy/fmt、SDK和193源hash核验通过。[最终证据](validation/windows-2026-10-10-console.json)保留初次拒绝及最终通过记录。
