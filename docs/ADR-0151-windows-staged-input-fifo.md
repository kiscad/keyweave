# ADR-0151：Windows 阶段会话有界输入 FIFO 与预算续排

阶段模式把已由 HookDispatcher 解析的 key 与 provider context 存入最多 128 条的 FIFO，按观察时间推进 controller 逻辑时钟；output pump 使用实际会话时间。stage poll 的 cutoff 不超过最早等待输入，确保早 Up/context 不被晚 tick 越过。每次 callback/tick 至多 64 项工作，预算用尽正常保留 pending、next_wakeup 立即提示续排；Legacy 保持原预算错误行为。

新的按键在其有界队列接受后返回 Capture；这不是输出已接受证明。repeat 在对应输入被处理且先前 output 交付后仍核保存叶与 desired/submitted。context 入队前立即验证大小/字段/修订/设备与时效，避免未处理的大快照进入内存。FIFO 满或实际 stage/output 错误仍 StopRequired，卸钩后按原独立 untracked/access/accepted 事实恢复或隔离。

未处理输入存在时 activate 冷拒绝 StagedInputPending、不改变模式/config/队列，也不声明 Applied；已登记旧 root 的未来 stage work 不阻止正常 reload。Reset/stop 丢未处理输入并清 stage，恢复只看 accepted ledger。此为显式阶段 actor，Windows 原生 worker 模式与平台验收仍待。

5 个新增 tests、更新旧 staged budget case、工作区 1383 用例/Clippy/fmt/source60 通过。512 tap 完成 1024 有序 packet，input seq 只数真实 key/context；晚 tick 保 key/context/key 选择次序。立即过期但字段合法的 context 仍可存储并在条件计算给 Unknown，不改既有三值语义；测试使用 future observed_at 验非法 metadata。

原生捕获 wrapper 仍须独立审查 untracked-input/access-loss；handler 失败后源被 Pass 会改变 global ownership，不能仅凭此 actor 的 mock accepted ledger 推断可安全全局释放。实际 Windows x64/i686 与 Hook 测试未执行，默认 native entry 仍旧模式。下一步应把固定 Windows execution-mode 的冷准备、argv、capture factory/reload 与监督一并贯通。
