# ADR-0056：Windows worker 的有界协作停止

## 问题与选择

已有 Supervisor.request_stop 是强制终止自有 Job。正常用户停止需要先让 worker 关闭捕获、有限恢复已知输出，再真正退出；Job kill 继续承担独立超时保护，不能被描述成恢复全局按键。新增共享协议的显式 opt-in StopRequest，Windows Worker 接收并转到既有 Requested/shutdown 路径，Supervisor.request_shutdown 负责有界发送和观察退出。完整 parent host 的命令入口、不可变配置交付和独立诊断/热更新仍另行开发。

不把 EOF 当正常停止，不用损坏挑战帧触发错误恢复，也不改旧 challenge/receipt v1 字节。Linux adapter/default client 仍走原 receive，不自动启用控制，遇到新 Stop 帧明确拒绝；只有已知支持该契约的 Windows worker 使用 receive_command。无第三条管道/新继承句柄/新 FFI，沿用已核验的 private owned ordered reader/writer；它不是跨用户 RPC 或新的身份认证。

## Wire 与接收顺序

StopRequest 与 Challenge 都是 24 bytes：magic=KWPS、version=1、三个 reserved bytes 为零、little endian 正 u64 run_id 与 id。KWPC/KWPR/24/40 的旧 bytes/golden 保持；旧 Challenge.decode 拒绝 KWPS。id 与挑战共用严格连续序号，必须为当前 run 的 last accepted request+1，不重试/回绕/接受旧帧或另一个 run。没有 Stop receipt：最终确认来自 owned process 真正 Exited，加上真实 worker 的退出分类；发送完成不证明收到了停止或已恢复输出。

opt-in receive_command 每次至多一次 Read，可在旧 ticket 或 partial reply 期间接收紧邻的 Stop。完整合法 Stop 立即锁存，不 mint epoch，不 poll Session，不再 publish/flush；旧 ticket/冻结 response 与 native 在途存储保留至 owner 清理/进程退出，不改造成终止帧。Windows Driver 先 callback gate，再 receive_command；Stop 转到 request_stop，先关闭捕获，再有限已知 ups 恢复。Requested/FramePending/NoFrame 不证明最终回应或全局清理；后续 step/run 缓存原报告、零 IO。

如果流中先有合法下一挑战，最多停放一个完整帧，直到旧 response 完成后才接受它并记录当时 Session epoch baseline。底层读完字节不等于协议接受，不能用提前读的 epoch 发回应。停放期间不再读第二帧；后面的 Stop 必须按 stream 顺序等待这个前序帧。此前期望 partial 时完全不 Read 的两个 Windows tests 改为允许这一次有界停放/stop-priority Read，冻结帧与下一 baseline=8/epoch=9 的原断言继续保留。未知 magic/reserved/version/错误 id 立即报协议错误，不能在 Standby 中静默停放 malformed 内容。

## 父监督状态与 deadline

request_shutdown(now) 只允许 callback 外。首次请求先检查现有 progress lease 与单调时间，然后保存 deadline=min(已有 lease deadline, now+2s)。加法显式 saturate 到 u64 上限，再由旧 lease 截断；不延长 lease、不以停止意图或旧 queued receipt 续租。重复请求仅检查同一 deadline/time，不重新计时。原强制 stop/故障已选定时，只重试其自有 Job 操作，保留首个 hard reason。id 耗尽立即强制 stop，不能 wrap。

SupervisorStatus 新增 ShutdownRequested{deadline,request_sent}。尚未尝试写的当前挑战可直接替换为同 id 的 Stop；已经尝试（包括 WouldBlock）必须完成完全相同的原挑战 shadow，随后排队下一 id 的 Stop。即使写返回 WouldBlock 而 written=0，也视作在途，不能替换。每 poll 至多一次 Write；完成原帧后只排队，下一 poll 才写 Stop。已完成写/等回应时可排下一 Stop，不继续读/续租原回应；已有 reader OVERLAPPED flight 的独占稳定存储保持到退出后的显式 close/cancel，不能因不再读取而释放正在由 OS 写入的内存。

request_sent 仅表示整帧获得 Write acknowledgement，不是退出或 cleanup。之后只 poll owned process（零 timeout）与固定退出期限；不读回应、不发 heartbeat。迟到/重复 request、旧 queued Active receipt、后台线程存活、Stopped 都不能延续期限。期限到达、clock 倒退或通道故障按原策略强制终止 owned Job，stop failure 后零 IO/不续租并可重试；进程查询错误继续保存主错误与 stop 次错误。

只在 signaled owned process 时 Exited。没有 hard reason 时 reason=Cooperative 表示请求意图，退出码仍须单独检查；即使 child 在收到 Stop 前自行退出，也不能把该 reason 当成 delivery acknowledgement。首个 hard reason 优先于 Cooperative。立即 request_stop 仍是原强制 Requested 路径，保持兼容；Drop、Job exit 不证明全局键清理。close_channels 仍必须观察真实 Exited 后收集两个独立结果。

## 验证与限制

共享 tests 覆盖严格/独立 wire 与 legacy 拒绝、partial stop 无 publication、Standby ticket/旧 Active partial 预先停止、停放挑战后的 acceptance baseline 与 Stop、不合法 run/id/reserved。Windows tests 使用实际 Session/actor/输出账本代码及 Win32 mocks，检查 passive 下零实际 poll/零输出、active partial 先关捕获再已知 up、错误 Stop 无新 poll；Supervisor mock 覆盖未写替换、pending/partial 原 shadow、旧回应不续租、2s/旧 lease deadline、clock、Stopped/部分 read、stop failure retry、id 耗尽/非法 Write count/WriteZero；owned wrapper callback 测试不改状态，生产 native_link 保留 request_shutdown 地址。

check_cooperative.exe 真实自有进程例程：initial 先发 Stop、active fixture reply 后协作停止、standby 不回应仍读 Stop、读 Stop 后停滞与startup 不读；全部通过真实 WorkerStartup，等 signaled process 后清理父管道。例程 epoch 是明确的 synthetic fixture 数值，不是 CaptureSession poll；不安装 Hook、SendInput、driver 或读取真实键盘。正常捕获停止/恢复、Hook 活性、其他来源全局按键共存、Windows11/真机与完整 host 仍未验收；原先 x86 native 异常原因保留，不以此消除。Linux 实际 worker bootstrap/consumer-ready、D05/GUI/安装/Rust driver/companion 审计及全部原目标继续。
