# ADR-0090：独立热配置请求、激活确认与活动代号查询

日期：2026-10-11。状态：纯协议与 actor 查询已实现；原生 transport/CLI 尚未连接。

## 协议与顺序

`keyweave_progress::reload` 是独立控制协议，不能写入既有 progress/Stop 两条私有流，也不修改其 challenge/receipt/epoch。该模块没有配置解析、OS 调用、原生端点或 actor 激活实现。

request 与 ack 都是48字节、little-endian、version1。前8字节为 magic/version/kind/两字节零reserved。request magic KWRQ、kind0，随后五个 u64 是run、request id、base generation、target generation、payload字节数；ack magic KWRA、kind1/2/3分别Accepted/Rejected/Cancelled，随后是run、id、base、target、actor active generation。严格拒绝未知version/kind、非零reserved、错误长度、零身份/代号、非base+1的target和1..8MiB之外payload。

该长度上限与配置编译器MAX_CONFIG_BYTES由集成测试核对。协议尚不传输或校验body，不包含digest、路径、FD或handle，也不认证peer；未来仅用于本会话拥有的可信私有端点。

## 父端与 worker 状态

Client只有一个pending，request id从1递增，无Clone和全局计数器。begin成功才保留原请求与固定deadline；Busy不重发或延长。显式timeout在1ns..30s以内；相等deadline已经过期。倒退、溢出、超时或无效/重复/错run/id/generation ack永久关闭lane，不推进active generation。无效候选长度不消耗id。

仅Accepted且匹配pending的ack推进父端active；Rejected/Cancelled保持base。准备、发出、字节接受和旧持键释放都不是ack。Stop/session loss/IO fault关闭lane，不在同run重开；ack丢失时worker可能已commit，不能盲目重试或宣称回滚。

Worker校验连续id、run、base与当前active以及target。finish必须在实际actor.activate结果之后，由可信调用方提供actor当前代号：Accepted必须target，Rejected/Cancelled必须base，否则关闭lane且无确认。该primitive不能阻止调用方谎报actor状态，原生集成仍须把调用位置锁在实际激活结果之后。commit可先于ack写入，故ack IO失败必须停止会话并保留不确定性。

Client/Worker不是进度监督器；它们不含poll epoch、延长父lease或证明Runtime存活。候选准备与transport需要独立预算、独立端点，Stop/session/progress fault优先。

## 活动代号查询

Engine、Linux Runtime与Windows SessionActor新增只读config_generation，返回当前已准入快照，独立于held bindings的旧代号。它不造poll或消费者授权，不证明actor当前Active。

Windows CaptureSession查询与activate_config均拒绝callback、closed session或actor借用冲突，使用try_borrow/try_borrow_mut返回ActorBorrow，避免配置控制路径的RefCell panic。Callback-context gate只读取元数据，不发native调用；有效激活继续原有validate与Engine.activate。关闭查询不被当作可重启授权。

## 验证与下一步

见 [验证记录](validation/linux-2026-10-11-reload-protocol.json)。八个协议tests覆盖wire布局/reserved、单在途/拒绝重试新id、错run/id/generation、伪造actor active、严格超时/倒退/Stop、worker重复/跳id、资源耗尽与safe Debug。Request/Ack Debug不打印run identity。

portable集成测试把两平台冷factory→Engine实际activate→Worker.finish→wire decode→Client.accept串起来，证明candidate不提前推进代号、ack后held旧binding仍按原键up、新down按新快照。native mock test覆盖callback与actor borrow拒绝、无输出/状态变更，成功查询和closed拒绝；已有Linux hot-config test补查当前活动代号。

Linux886 tests、Clippy、fmt、60来源检查通过；测试在2026-10-10开始，记录于2026-10-11，日志目录保留原日期。无新FFI/unsafe/dependency，C oracle未重跑。Windows新协议/actor/query及前序待补native checks仍未在Windows执行，不能用Linux mock结果替代。

接下来实现有界header/body/ack传输、冷候选准备任务、实际actor控制交付和确认，然后接原生端点、host watcher及reload命令。仍未提供可运行的热配置功能，完整跨平台目标未完成。
