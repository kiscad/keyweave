# ADR-0148：Linux 运行时阶段控制器模式

Runtime 新增显式 new_staged/preflight_staged_config，单一 Box machine 复用捕获所有权、输入中立/bootstrap 前提、kernel repeat、OutputPump、accepted/REPORT 账本、移除设备及有限恢复。默认构造与 native bootstrap/worker 准备沿旧 gate，本 ADR 不切换真实入口。

每次 poll 原 64 项工作预算中，先 output due，再一个 stage poll，再 legacy timer/已保存物理事件。达到预算正常返回 pending，后续 poll 继续，不能重复注册事件或伪造 timer token/seq。原始输入/normalizer/current-delivery clock 流程不变。阶段异常走原先 capture-close-before-output-recovery，Reset intents 不当接受账本。

EngineState 保持已有接口；阶段模式 bindings/待完成根/generation 取真实物理根，output_owners/pressed_outputs 取实际 Terminal 模型，pending_deadlines 数独立 queued stage work 和 held/delayed tasks（Output 生成 mirror 不重计）。这些均不是 native submitted/目标应用状态。backend 仅 keyboard frames，阶段 pointer 不支持。

必须用 mock FrameBackend 证明 duration、预算续排、早 Up、热更新、短输出/REPORT/未知接受及设备移除；平台 bootstrap/原生 consumer-ready、pointer 与完整入口随后完成。

8 个新运行时测试和 1 个状态测试、工作区 1367 用例/Clippy/fmt/source60 通过。512 tap 以 1024 个有序键边跨有界 poll 完成；REPORT/unknown 与跨设备 scope 都有精确 native frame/mock receipt 断言。kernel repeat 沿原策略委托，逻辑 earliest/mock 接受计数不证明真实应用间隔或系统重复行为。

真实入口仍由默认 legacy 预备选择，未进行 evdev/uinput 操作。下一步应将明确 execution mode 的冷 config/bootstrap/activation/reload 凭据贯通，保持输入捕获关闭与独立终止报告；不能只替换一个 constructor 绕过 worker 的准备 gate。native pointer、Windows 异步 drain、真实平台验收及完整目标继续。
