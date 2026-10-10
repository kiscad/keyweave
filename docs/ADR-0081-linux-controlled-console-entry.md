# ADR-0081：受控启动等待与 Linux console 实际 main

日期：2026-10-10。状态：main 代码与原生组件连接已实现；实际捕获成功路径未验，Linux host 待实现。

## 实际行为

新增显式私有参数格式：`--keyweave-linux-worker <parent> <run> --uinput <absolute-node> --session-provider console --vt /dev/ttyN --input <absolute-node> ...`。它只允许 canonical tty1..63，1..64输入；参数最多137个，迭代最多取138个检查overflow。路径仍为 OsString/PathBuf，无损保留非Unicode输入路径；VT自身要求固定ASCII格式，不通过host平台absolute规则解释Linux VT字符串。Debug不输出路径/parent/run。

原 `--session-provider none` 合约永久不操作设备，仍经父/run/三管道/sealed config冷准入后返回78。它拒绝console/VT选项，保持64输入限额。不能将其有效配置真实exe测试改为console。Legacy Windows格式在Linux仍返回Unsupported，Windowsmain执行路径不变。

console main在真实初始线程完成ReportingStartup、parent/run比对、单次sealed config claim、position read和严格冷编译后，调用run_reporting_console_session。显式输入按argv顺序分配1..64的临时DeviceInstanceId/generation1；它不是持久设备选择/跨run身份，进程内新Engine和fresh run监督不能接前一run的timers。

## 启动控制与交付

session消费ReportingStartup得到guard、同一progress client和terminal writer，再核验guard/端点非阻塞flags。首次receive_startup_command在构造只读VT/provider之前执行，已缓冲Stop直接取消。之后每个bootstrap step前读一次控制；后到Stop是合作检查点，不是禁止下一syscall的原子承诺。

startup receiver仅以baseline epoch0保存challenge，从不publish“metadata心跳”。private drive_controlled消费bootstrap，Pending时监听两条progress管道与原启动期限，使用原clock和housekeeping20ms上限；不读输入/不造进度。不新开stdout日志，不延长parent lease，不增加无限sleep/retry。管道HUP/error/invalid按Endpoint失败；不会将其当成capture成功。

Ready交付原Runtime/provider/clock/ready给ADR-0080 controlled console loop，同一个progress client保留未完成challenge；第一次实际Runtime.poll的epoch1才可回答baseline0。该循环继续Stop优先、首次重新查consumer、active续期与失效capture-first恢复。完整run之后才用既有bounded KWLT终止发送。

driver的Stop/错误消费其唯一bootstrap owner，因此partial-open及Captured但未交付Runtime的状态都走ADR-0079 input→output→provider关闭。预Runtime Stop返回StartupStopped：main标量0，但无KWLT Runtime报告；这只说明无Runtime dispatch，不是独立ungrab/全局恢复证明，host必须按“启动取消/缺少Runtime报告”独立审查。捕获后的原生Drop也不等于报告已确认。

## Linux退出码

64参数，65父/run/管道/sealed claim准入，66冷配置，67bootstrap/device/provider安装与期限错误，68启动协议/endpoint或worker loop错误。完整运行的终止事实或终止发送不完整为69，优先于其他标量；已送frame仍有delivery failure也失败。只有完整独立facts/发送结果、outcome和termination reason一致且为Pause/Emergency才返回0。Loop error不能被完整恢复覆盖，Pause/Emergency相互矛盾也不能Clean。所有原事实仍在report对象/KWLT中保留，不把Windows高位mask截成Linux8bit。

stdout/stderr/stdin继续专用于二进制协议直到真实process exit；没有错误文本或panic日志。实际退出由execute所有owners返回/Drop后main完成；标量或KWLT完整都不代表image认证或全局按键处理完成。

## 验证

Linux Rust1.97.1：完整workspace all-targets833tests、Clippy -D warnings、fmt、60-source verifier通过。新增3个mock driver/protocol测试（Stop在create前及Captured后、challenge跨handoff且仅真实poll发receipt、协议/wait故障清理），2个portable args测试（VT/路径隐私、两模式64界限），2个纯main退出分类测试（独立delivery/recovery与loop矛盾），1个真实exe console缺guard拒绝测试。原none真实ownedSupervisor冷配置66/65/78检查仍通过。

真实exe的console测试**必须缺少父保护**，结果65证明没有进入设备事务；它不是successful捕获测试。成功bootstrap/停机仍由mock测试支撑，不以这些结果宣称WSL Terminal或真机可用。未查询真实VT/访问evdev/uinput、Hook/SendInput或驱动。首次测试编译因测试helper名称遮蔽及Receipt字段名错误失败，已修正测试，不放松生产准入；初步失败日志保留。[验证记录](validation/linux-2026-10-10-console-entry.json)

无新FFI/unsafe/UAPI，C ABI oracle不变、本轮未重跑。新portable参数测试仍需Windows本机补验，ADR-0073/0074等前序Windows原生检查仍受此前自动审批额度故障影响，本轮未调用Windows。

## 剩余与验收边界

下一步实现Linux host：冷read/compile/seal、fresh run identity、可信worker定位、owned Supervisor spawn_reporting_config、独立poll/Stop/有界actualExit观察、KWLT与清理/错误报告。parent startup预算包括worker冷准备及constructor，不能因bootstrap首step+10s期限而延长；没有startup假心跳来掩盖未就绪。host的signal/进程组/控制TTY、睡眠/会话撤销仍需落实，不能让用户Ctrl+C意外先杀child而跳过恢复。

专用真机验证需要显式环境授权与可恢复通路，不在普通自动测试中运行有效console配置的真实capture worker。图形桌面/PTYS/WSL Terminal均仍不属于console provider；动态VT/handler变化不原子，TTL/有界字节不证明syscall期限，端到端交付/全局恢复及suspend仍未证明。

高级多阶段规则、simultaneous、hotplug/add、LED、热配置、GUI、打包、Rust driver与companion审计等完整目标继续未完成。
