# ADR-0209：Windows mixed FIFO 与按钮接受恢复

独立 MixedOutputPump 连接原 OutputQueue 与显式 SendMixedBackend：键盘和五按钮共用 desired/submitted 账本，motion/wheel 为原序独立 barrier。一次 poll 只一次有界调用，不能跨 impulse 合并 key span。全部 enqueue 先验原生编码/容量再改变队列，时间沿 FIFO 非递减归一。所有工作总8192，单次提交最多256。默认键盘 Runtime 与能力不变，后续 capture/runtime 显式采用新 pump。

正常短接受进入 StopRequired，调用者先关闭捕获再 begin_recovery；丢弃所有未执行工作，只沿同账本为真实接受的键/按钮发 Up。移动/滚轮只消费该调用、从不重放，包括零接受。Rejected 仅 native 调用前，按0接受处理；未知或非法count quarantine、保最后已知 submitted 且禁止恢复注入。close/drop backend不清 global state、不制造中立证明。恢复短接受可继续有限poll，但不能重放已接受Up；Runtime负责既有次数/时间预算。

SendInputBackend 显式实现混合打包及全batch/tag预检，复用既有一次同步API函数；不同事件类型可保同队列序。Microsoft [SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) serial insertion/count 语义沿用 ADR-0208。Linux mock 只证明codec/FIFO/ledger，不证明 Windows-only实现编译、UIPI、实际hook或delivery。
