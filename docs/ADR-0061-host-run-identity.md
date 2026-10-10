# ADR-0061：Windows host 每次启动的随机 run ID 与注入 marker

完整 host 不能依靠 PID、时间、计数器或固定 marker 区分启动实例。新增 RunIdentity::generate 冷准备 API：callback 外使用 BCryptGenRandom 的 system-preferred RNG，读取12字节，明确以 little endian 解码 u64 run ID 和 u32 marker；两者都必须非零，最多四次完整抽样。失败返回原 NTSTATUS、没有弱随机 fallback；连续零值返回 Exhausted。每次 host 新建 worker 都应重新生成，进程重试不得复用旧 run/config/progress 状态。

类型只读，未实现 Clone/Copy；getter 供固定参数与 Supervisor 传值，Debug 不显示数值。它不是认证凭据、不保证绝对唯一，32bit marker 存在随机碰撞；不把 own-injected 分类用于跨用户鉴权。两架构 marker 使用同一 u32 范围，固定私有 argv 接受，绝不截断 usize 值。调用只取得系统随机数，不创建进程、文件、管道、输入、后台线程或 timer，不放到 active watchdog/keyboard callback。

真实 BCrypt 原型沿用已有 SDK oracle；本模块独立窄 FFI，mock 保留参数与返回值证据，覆盖 callback 零调用、精确字节解码/宽度、零值重抽/耗尽、首/后续失败和 Debug redaction。生产集成只生成身份并检查格式/宽度与固定 worker argv roundtrip，无 Hook/SendInput/worker spawn。随机值的不同不能作为确定性必然保证，测试不依赖统计碰撞概率。

下一步可信 worker exe 定位与完整 host 控制/启动事务；PreparedConfig→fresh RunIdentity→seal→Supervisor.spawn_with_config，启动后独立控制线程驱动实际 progress/有限协作 stop/真实 Exited 和独立清理。原目标全部保留。


实现与验收：Linux690、Windows x64/i686各659 tests、alltargetsClippy/fmt/SDK/source60通过，两个182源hash匹配。真实系统RNG与固定worker argv roundtrip无输入；[详细证据与限制](validation/windows-2026-10-10-run-identity.json)。
