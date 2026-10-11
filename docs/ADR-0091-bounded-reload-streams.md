# ADR-0091：有界热配置 header/body/ack 传输

日期：2026-10-11。状态：portable streams已实现；native adapters与运行中reload尚未连接。

## 组件与预算

`reload::transport`沿用独立reload协议。RequestWriter保留固定48-byte头与owned候选正文；Parent持有唯一Client和固定ack缓冲。每次write/read poll最多一次IO，正文slice最多64KiB，头和ack最多48bytes；完成头部的同一次poll不再执行正文IO。不flush、不sleep、不在内部重试Interrupted/WouldBlock。

Parent.start从owned body长度创建请求和writer，避免独立长度与候选不一致；Busy/坏候选保留已有pending。显式RequestWriter.new仍校验严格header/body长度，失败后调用方不得继续一个无writer的pending事务。Parent在IO前后都读取注入clock并检查原Client deadline；后时钟失败、倒退或过期，即使最后一字节或完整ack已经读写也仍失败并关闭lane。时间不续租、不用于progress。

Parent没有pending时不读取ack。只有完整合法ack、匹配pending且post-IO deadline有效，才更新观察到的代号。RequestWriter完成仅是本地写接受，未证明worker解析、激活或确认。

## worker正文与确认门控

Receiver只读固定头，decode并由Worker验证run/id/base/target后，才try_reserve受限的正文。正文分段填充，最多8MiB；未完成不对外暴露候选。take仅一次，移动原始Vec，不clone或解析配置，也不输出body。之后读取停止在AwaitingCommit，避免第二个候选越过实际actor控制。

finish必须在实际actor结果后交付Accepted/Rejected/Cancelled及当前代号；保留精确ack，进入AwaitingAck。AckWriter部分写固定帧，完整写入后只发行一次不可由外部构造、不可Clone/Copy的AckSent。Receiver核对token中的完整ack再清头状态、允许下一事务。它证明本地Write接受，仍不证明peer receipt或actor真伪；native调用位置/端点绑定有责任提供这些前提。

## 故障与原生边界

partial/WouldBlock/Interrupted保留prefix与deadline。EOF、zero write、超出slice的IO count、身份错误、clock/协议错误使相应lane永久logical close。正文释放，未来poll不再IO；已commit的active不回滚。close并不直接发送Stop或证明进程退出；owned native资源实际关闭由其Drop/owner完成。候选或确认失败必须由未来控制循环转换为原有优先停止路径。

Generic Read/Write不证明nonblocking、方向、端点身份或权限。native adapters必须使用该run独占的另两条reload端点，不能复用progress/Stop；Windows需要真正的owned async buffer/completion，Linux须核验nonblocking/FD身份。Worker Receiver和AckWriter没有自己的时钟，外围必须在IO前后检查独立parent lease、session与停止控制。分配及syscall耗时没有硬实时上界；正文收集/候选编译禁止放在input callback中。

## 验证与后续

[验证记录](validation/linux-2026-10-11-reload-streams.json)保存九个portable scripted-stream tests：分段header/body/ack、每poll单IO与cap、收到正文后等待commit和token、错header先于分配、partial EOF释放、最后write/read后的严格过期、WouldBlock/Interrupted不重置状态、overcount/zero/clock故障后不重试、错token不回滚，以及第二事务重用干净头状态/拒绝保持已commit代号。没有文件、进程、native input/output或驱动动作。

Linux895tests、Clippy、fmt、60来源检查通过；无新unsafe/FFI/dependency，C oracle未重跑。九个新portable tests及前序Windows变化仍待native检查。

下一步把Received送入有界冷准备任务，以actor实际激活结果生成finish/ack；再连接原生端点、parent/worker循环与host watcher/reload命令。完整跨平台目标尚未完成。
