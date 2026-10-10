# ADR-0078：Linux 文本控制台的有限就绪证据

日期：2026-10-10。状态：组件及初始准入 gate 已实现；实际捕获 worker/活动续期尚未连接。

## 问题与范围

已有 OutputNodeReady 只能证明 owned uinput 有匹配的 evdev 节点与能力。它不证明消费者存在。不能用固定 sleep、节点存在、写入成功、udev settle 或任意应用进程存在代替 consumer-ready。

本切片提供 `console::ConsoleProvider`，仅面向内核文本 VT。图形桌面、SSH/PTYS、WSL Terminal 和 remote/seat 需要独立 provider，不能借用本证据。真实 main 目前仍只接受永久无设备操作的 `session-provider=none`，未开放可抓取 console 参数。

## 原生观察

`open` 限定 canonical `/dev/tty1`–`/dev/tty63`，以 read-only、CLOEXEC、NOFOLLOW、NONBLOCK、NOCTTY 打开唯一 File，核验字符设备 major 4/minor 与选定 VT 一致。打开前及每次 observe 重验 WorkerParentGuard，继续要求受父保护的 worker 主线程；owner 为 !Send/!Sync。禁止调用 setsid/获取控制台、激活或更换 VT、读取用户输入、改变模式或前台进程组。

以 VT_GETSTATE、KDGETMODE、KDGKBMODE、tcgetsid、tcgetpgrp、getsid/getpgid 读取元数据。必须满足：选定 VT 活动，KD_TEXT，K_XLATE 或 K_UNICODE；控制终端 SID 为正且等于 parent/worker SID；终端前台 PGID 为正且等于 parent PGID。首次 SID/前台 PGID 被锚定，后续不能自动将另一个会话/前台组当作原会话。未要求 worker PGID 等于前台 PGID，以便未来将 child 隔离到独立组而仍监督原前台 host；该组隔离本轮没有实现。每个失败 syscall 立即保存错误，不让后续查询覆盖 errno。

内核的 TIOCGSID 准入检查要求调用者拥有该控制终端，这支持上述严格范围；root 身份不意味着可给任意 VT 发放本证据。[Linux tty_jobctrl.c](https://raw.githubusercontent.com/torvalds/linux/master/drivers/tty/tty_jobctrl.c)

`observe` 只接受处于 Ready 的 owned UinputDevice，以其实际 sysfs input identity 精确查 `/proc/bus/input/devices`。库存上限 1 MiB+1 检查，必须完整结束，不含 NUL；重复身份、重复 S/H 元数据、匹配项缺 handler 都拒绝。只认该 identity 的独立 `kbd` token，不认其他设备或近似名称。identity 限定 `/sys/devices/virtual/input/inputN` canonical 十进制后缀。查询前后均要求其 `inhibited` 精确为 `0\n`，VT/会话样本前后一致。

内核 kbd_connect 注册名为 kbd 的 handle 并打开输入设备，打开失败撤销注册；proc 库存记录 Sysfs identity 与已连接 handle 名称。这给出有限的消费者连接证据。[Linux keyboard.c](https://raw.githubusercontent.com/torvalds/linux/master/drivers/tty/vt/keyboard.c)、[Linux input.c](https://raw.githubusercontent.com/torvalds/linux/master/drivers/input/input.c)

以上官方 master 源码于本日核验，用于说明接口路径，并非本机自定义 6.18.33.2 内核源码的逐行审计。Rust 文件是独立实现，没有复制内核函数。

## 时间与准入

观察期限固定 250ms，从查询**之前**的 CLOCK_MONOTONIC 时间起算。查询后读取同一 clock；倒退/溢出返回错误，迟到或任一条件不满足返回 None。`fresh_for` 同时要求 owned output path 匹配与 `observed <= now < until`。ConsoleReady 私有生产构造，无 Clone/Copy，Debug 不输出路径；测试工厂只在 cfg(test) 存在。没有用查询结束时刻延长有效期。

`CaptureGroup::try_arm_console` 在 Standby 阶段先检查证据，过期/不同输出返回 ConsoleNotReady，保留 Standby，零 neutral 查询和 grab。通过后走已有整组能力/紧急组合/neutral/rollback 事务。旧 try_arm 的 caller 就绪责任保持。

**这只是初始 gate。**调用者提供真实当前时间，并须在 arm 后、Runtime 首次 dispatch 前重新观察；活动循环持续续期，失去证据即先关闭 capture，再走既有有限恢复。该后续连接仍待做。不能把 initial gate 的成功当作所有抓取查询完成后仍新鲜，不能把 ConsoleReady 当作监督/进度 lease。

## 限制与故障契约

前后样本不是原子快照，不排除 ABA；在两次读取之间切换会话/VT/inhibit 后恢复仍可能不可见。handler membership 不证明端到端处理：外部 EVIOCGRAB、keyboard notifier、消费者关闭或新状态都可能影响交付。输出 accepted/submitted 账本仍只是已知内核接受量，不证明终端/应用实际处理，也不承诺全局按键状态恢复。

文件/ioctl/query 的 O_NONBLOCK 和长度限制不证明 syscall 在固定时间内返回。独立父监督与 parent-death 保护仍必须先建立；不能让本 provider 自己生成进度 epoch。CLOCK_MONOTONIC 不含 suspend 时间；当前切片不增加睡眠/恢复正确性承诺。后续 host/controller 必须显式处理会话/睡眠撤销，不能仅靠本 TTL。

错误/None 不允许沿用旧证据，handler 未就绪不得抓取。生产路径不输出 proc 库存、键值或配置内容。File 的 RAII 关闭是 owner 收尾，当前没有将 close 或元数据查询成功当成全局 ungrab/恢复报告。

## 验证与下一工作单

Linux Rust 1.97.1：完整 all-targets workspace 808 tests、Clippy -D warnings、fmt、60 个固定上游哈希与扩展只读 C ABI oracle 通过。新增 6 个 provider tests、2 个 capture-group gate tests，覆盖精确 handler、损坏/歧义/截断库存、模式/活动/会话/前台组、时间与输出 scope、zero-IO 拒绝、既有 neutral 事务。真实 memfd regular FD 被拒绝，未向真实 VT ioctl，未读用户控制台或访问 evdev/uinput。初次 test fixture 类型错误、C oracle 缺 termios.h、Clippy 检查问题已修复；原失败日志保留在独立 preliminary 文件。[验证记录](validation/linux-2026-10-10-console-ready.json)

Windows 原生验证本轮未执行；ADR-0073/0074/0077 的待补原生检查仍保留，不能用旧报告替代。本轮 provider/准入新 API 均 Linux cfg。

下一步依序：

1. 实现可注入且有界的 Linux bootstrap，父保护/冷配置/监督端点先于设备；owned uinput 创建→节点发现/能力证明→console 观察→整组 neutral arm→再观察→Runtime 交付，失败严格关闭 input 先于 output。
2. 在 worker 控制循环加入 provider 定时刷新、失效撤销、真实当前时间，保持同一 clock epoch；刷新不能伪造 Runtime 实际 poll 的 progress_epoch。
3. 只有完整连接后才新增显式 console CLI 格式。保留 none 模式永久无设备 invariant；有效配置 exe 测试仍只用 none，捕获成功路径用 mock bootstrap，专用真机另验。
4. Linux host 冷准备/封存/身份/独立监督/stop/KWLT 观察与实际 Exit 报告；补故障注入和专用硬件验证。图形桌面 provider 独立实现，不扩大当前 console grant。

完整规则、多平台 host/session、GUI、热插拔/add、LED、Rust driver、打包与 companion 审计等原目标继续未完成。
