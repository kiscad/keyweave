# ADR-0147：Windows 会话 actor 的阶段控制器模式

共享运行时目前只驱动旧 Engine，不能交付分阶段数组持续时间。SessionActor 增加显式 new_staged/preflight_staged_config，复用同一输出 pump、accepted ledger、64 工作预算、stop-required 和 unhook-first 恢复；默认构造与原生 worker 冷准备仍沿旧 Engine gate。本阶段未切换真实 capture 入口。

单一内部 machine enum 拥有 Legacy 或 Staged 控制器，不能并行运行两套 owner。原生 due 顺序仍先 output pump，再一个阶段 poll，最后 legacy timer；阶段 poll 不伪造外部 timer token/输入 seq。physical repeat 只查询保存实际叶，并继续要求 desired/submitted 同时持有才发送 Repeat。阶段错误触发 stop；退出先卸钩再清 stage 未来工作、按接受账本恢复，不把 controller reset intents 当 accepted ledger。

提供控制器独立 reset/query API，reset 不消耗输入 seq、不复用来源/批次身份。当前 SendBackend 只有 keyboard packets，所以该模式不提供 pointer capture/forwarding，也拒绝意外 pointer receipt。旧默认 native timing gate 保留，未来原生入口必须选择经验证模式并补平台编译/监督/输入验收。

两种 machine 均在冷构造时 Box 独占持有，避免 enum 的 2680/600 字节差异；不使用 lint suppression。13 个新会话测试和 1 个控制器生命周期测试、工作区 1358 用例通过，Clippy/fmt/source60 通过。默认 native worker 路径未切换；原生 Windows x64/i686、真实 Hook/SendInput 和键盘验收仍未完成。

逻辑 earliest 时间不证明真实 API/应用的精确间隔。大型即时数组可能耗尽原 64-work callback 预算并进入 StopRequired；后续需把有界异步 drain 与原生捕获/lifecycle/条件时间契约完整接入，不能为通过单个 mock 用例解除默认 gate。跨设备身份仍是 Hook 的 aggregate STREAM，device conditions 继续拒绝。高级 Simple 分支、native pointer 与完整 Linux 阶段运行时仍待开发。
