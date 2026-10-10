# ADR-0086：Linux 原生信号与 Supervisor 驱动循环

日期：2026-10-10。状态：已实现；cold host 组合与 CLI 待接入。

## 决策与顺序

`NativeSession` 持有同一个 Session、HostSignals 和已观察 spawn 时间的 MonotonicClock。caller 须先安装信号 guard、prepare_child，再 isolated sealed-config spawn；构造时缺少 clock observation 立即进入 control fault/owned stop，不把零时间伪装为有效 spawn lease。

每步先采样时间、消费最多一个信号，然后驱动 controller。时间错误或倒退进入首 control fault、请求 owned 强制停机，仅用最后接受的 MonoTime 再观察退出，不用 std Instant 生成假 progress 时间。正常等待 signalfd 至多5ms；fault 后以5ms sleep代替故障 FD 等待，避免错误循环持续占用 CPU。收到信号或 readiness 均不能续 worker lease。

正常 Active 无任意寿命限制。首次 fault/ShutdownRequested/KillRequested 后，以独立 std Instant elapsed 开始五秒的退出观察。该时间只限制 host 观察，不重写原 Supervisor 的 progress/shutdown deadline，也不是 suspend/resume 证明。

## 观察超时与关闭

`ObservationExpired` 只表示没有在预算内观察到退出。NativeSession 保留同一个 owned child、协议端点、signal guard 与 clock，不读 KWLT、不关闭信号 guard、不重置观察预算。caller 可以继续 `poll_once` 或 `run` 检查同一个 owner；实际 exit 检查优先于过期判断，所以后续真实退出仍能被观察。不能因观察超时重新 spawn 或宣称已终止。

只有 actual Exited 才 collect terminal 并关闭 signal guard。NativeReport 分别保存原 worker Report 与 signal_close_failure；成功还须没有 mask restoration 错误。重复调用返回缓存，不重复关闭或观察。

结构字段关闭顺序把 controller/owned child 放在 signal guard 之前，供异常 Drop 先做原 Supervisor 的有限 kill/reap fallback。但这种 fallback 不证明 child 已退出；原 guard Drop 也不是全局恢复保证。正常路径必须获得 actual exit。线程、syscall 与 OS 调度耗时没有硬实时上界。

## 验证

见 [独立记录](validation/linux-2026-10-10-host-pump.json)。六个注入测试覆盖 Active 长寿命、过期保留 owner/不读 terminal/不关 guard、后续同 owner 实际退出、clock error 与倒退、receive/wait fault、首 Stop 不重复及独立 close failure/cache。

实际 main 无输入 harness 使用真实 MonotonicClock、sealed config、isolated child 与父死亡 guard。安装 host 信号后，只向该独立测试进程自身 PID 排队 SIGINT；原生 pump 收到后发送私有 Stop，观察真实退出并分别处理 complete/missing synthetic KWLT，最后恢复掩码。所有输入设备均未打开；没有向 terminal foreground group 发信号。synthetic facts 只证明传输与分类，不能称为真实 Runtime/hardware recovery。

全套 Linux tests、Clippy、fmt 与60来源检查通过。没有新 FFI/dependency，C oracle 本切片未重跑。Windows 既有原生待补验证仍未执行。

## 下一步

组合 cold config、run identity、可信 worker locator、固定 argv、密封映像与 isolated spawn，连接 `run-linux` CLI。命令输出须保留 scalar exit、actual signal/监督原因、KWLT 与 close failure，不打印配置内容/输入事件。保持 None 有效配置 exe 测试永久无设备，console 成功路径另做授权真机验收。完整跨平台目标尚未完成。
