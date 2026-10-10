# ADR-0087：Linux 冷启动事务与 run-linux 命令

日期：2026-10-10。状态：源码入口已连接；真实控制台/设备验收待执行。

## 决策与冷阶段

`PreparedLaunch::prepare` 先限制1..64输入并用真实 LinuxArguments builder/parser 做 lexical preflight；此处 run=1 只是参数验证值，不作为 RNG fallback 或实际 spawn identity。此后定位可信 worker，打开并检查 regular 配置，使用完整 output table 冷编译/validate/Engine 准备，生成一次 fresh RunIdentity，构建实际 argv，并密封完全相同的原始字节。传给 Command 的是 OsStrings，不 join shell，不丢非 Unicode 设备路径。PreparedLaunch 不可 Clone，只能消费一次。

配置仅在此读取，start/活动循环不重开 pathname。文件 regular 检查发生于 open 后；cold 文件系统操作没有硬时间上界或原子路径身份保证。locator 不做 PATH/shell 搜索；canonical regular/xbit 检查不是签名、ELF、有效 ACL 或 check-to-exec 原子认证。默认 packaged image/developer image 都有部署信任前提。

## 启动阶段

`start` 在实际 main thread 安装 HostSignals，先消费一个 pre-spawn Stop；有 Stop 时释放 artifact、关闭信号 guard，返回 Cancelled/no child。其他情况 prepare_child 恢复 child mask、观察 MonotonicClock 的 spawn 时间、调用 isolated sealed reporting Supervisor，再用同一个 clock/control owner 进入 NativeSession。启动错误显式关闭 guard并独立保留关闭失败；spawn 成功后继续持有同一个 child。

取消表示没有启动该 worker，不构成全局恢复证明。三个 managed stop signals 之外的信号仍按原系统行为处理。caller 不能把该接口用于已有任意多线程/handler 的 embedding。

## 命令与诊断

`run-linux` 明确要求 config、输入路径和 `--session-provider none|console`，uinput 默认 `/dev/uinput`，显式 worker 可选。console 必须提供 canonical literal VT；None 禁止 VT，并永久禁止设备操作。非 Linux 命令返回78，不读取配置或启动 child。当前没有隐式默认 desktop provider、自动选键盘、切 VT 或提权。

成功判定沿用 NativeReport 的实际 exit、监督原因、独立 KWLT facts/host/close failure；缺 frame 即使 worker code0 也不当作恢复完成。观察超时打印 unobserved/failure，CLI 随后采用原 owned emergency Drop，不把它当 actual exit；库 owner 可保留并继续观察。pre-spawn cancelled/no close failure 可返回0，表示未启动该会话。

StartupError 只保存 stage 与 SystemFailure，不保存/打印私有配置正文或路径。NativeReport 的 Display 分开打印 scalar code、退出 signal、监督/停止原因、epoch、terminal 状态、worker facts、host fault 和 mask close failure；避免 Debug 整个 TerminalFrame 而泄漏随机 run ID。public report 数据仍可由 caller 访问，库不能防止调用方自行格式化私有 identity。

## 验证

见 [独立记录](validation/linux-2026-10-10-run-command.json)。三 cold tests 验 exact sealed bytes/argv、坏参数先于 lookup、坏配置及目录拒绝；privacy test 检查 Display 不输出 frame identity；两个 CLI 测试确保 scope/VT/relative input 或 bad config 在 spawn 前失败。

actual-main no-input harness 完成真实 prepare/start：封存后删除原配置 pathname，child 从 inherited artifact 校验精确字节、父/run/组/mask 与 None argv，返回78/缺帧。fixture 是 synthetic 冷 worker，未冒充 Runtime 恢复。

随后实际构建 host 与 worker，执行唯一真实 `run-linux --session-provider none` 成功启动路径：真实 worker 按永久无设备契约返回78，host 返回1，terminal=missing/recovery_confirmed=false，stderr 为空。产物 SHA 与实际文本保存于记录。未运行 console 成功路径、查询 VT 或抓取/创建输入设备。

完整 workspace tests、Clippy、fmt、60上游来源检查通过。无新 unsafe/FFI/dependency，C oracle本切片未重跑。新 portable CLI 和非 Linux stub，以及既有 Windows 原生待补检查，仍未在 Windows 执行。

## 后续

按 [Linux 使用说明](LINUX_USAGE.zh-CN.md) 做独立授权的 VT/抓取/输出/释放/停机真机验收，补 suspend/session 控制和图形消费者。继续 advanced rules、设备热添加/LED、热配置、GUI、包装与 Rust 驱动。完整跨平台目标仍未完成。
