# ADR-0096：Linux reload 实际启动与收尾组合

日期：2026-10-11。状态：真实 worker main 与 console host 启动已连接；用户 watcher/reload 命令和成功硬件路径尚未完成。

## 显式端点模式

ReportingStartup.reload_requested 只判断两个专用环境选择器是否同时存在：均无时保留 legacy，均有时必须认领，只有一个时 admission 失败，不退回旧模式。语法、方向、NONBLOCK/FIFO/独立身份仍由 ADR-0093 claim_reload 单次校验完成；presence 不是认证或 readiness。

真实 worker execute_linux 在 guard/run/parent 检查之后、配置映像读取与设备 bootstrap 之前选择模式。reload 必须额外验证 isolated group 并认领端点，任何错误返回 admission=65。None 即使取得有效 reload pair/配置也仍返回 unsupported=78，永远不初始化 VT、evdev、uinput 或 Runtime，不发配置确认/虚假 terminal recovery。

console 模式构建新 Controller，绑定实际 startup run、已准备 snapshot generation=1、实际冷输出 spec，固定 reload 本地预算 5s。随后调用新 run_reporting_console_session_with_extension，使用与 legacy 相同的内部 guarded bootstrap/Stop/等待/Runtime handoff/terminal finalization helper；进入 ADR-0095 扩展活动循环。不会重新打开配置 pathname、替换 clock/provider/client、创建第二个 input actor 或复用 progress/Stop FD。

扩展 owner 由 main 保留。新 session wrapper 在 bootstrap 停止、错误和活动循环返回时都 close lane；活动循环已经先完成原 capture-first 输出恢复，报告仍保留独立 loop outcome 与 Runtime termination/delivery。StartupStopped 仍不编造 Runtime 报告，legacy 不改变接口行为。

## 冷任务收尾

terminal finalization 返回后，main 对同一 Controller 调用 reap_after_shutdown：close 锁存、不创建/取消重启线程，观察已存在 job，1ms housekeeping。每 owner 首次建立独立 Instant 的100ms窗口，重复调用不重置。已实际回收的终态可以在后续观察被承认；观察超时本身不证明任务结束，也不改变实际 input/output recovery 事实。线程调度/完成 join/睡眠没有硬时间上界。

不能回收或观察到 cold panic 时 main 返回 scalar cleanup=69，即使前面已发送完整 KWLT。host 的成功分类要求实际 exit code 0，因此不会用 terminal 中的恢复事实抹去此独立收尾失败。未结束 job 仍在原 owner 中，直到 main 返回/Drop；进程退出会结束所有线程，Drop/detach 自身不被称为终止证据。冷任务不持有 Runtime/设备或原生输出 owner，晚到 IR 永不激活/确认。

## Host 采用模式

PreparedLaunch 冷准备记录是否 console；start 对 console 使用 spawn_reporting_config_reload，None 使用原 spawn_reporting_config_isolated。默认 Policy 的 active lease=2s，host 持续用原实际 progress receipt 推进监督/下一 challenge，与 ADR-0095 本地父活动预算匹配；不存在按编译完成续租或新的 heartbeat。

parent reload endpoints 暂时保留在同一个 Supervisor 中，没有取走/写入候选，也不会因没有用户 reload 命令而丢掉 worker 端 peer。host/supervisor/Stop/signals/actual exit/terminal 的原 owner 不改变。本轮尚未提供 host reload 请求管理、file watcher、CLI 或任意外部控制 socket；因此不能仅凭启动选择新模式就称用户热更新功能完成。

## 验证与下一步

Linux 1.97.1 locked/offline workspace/all-targets 923 tests、Clippy -D warnings、fmt、60 项固定来源校验通过。新增 3 tests：两个选择器完整性/partial fail，后台 preparing owner 经收尾 helper 回收且无晚到激活/ack，真实 worker exe 的 None+有效 pair 返回78、None+错误FD/缺失 selector 返回65（三个 owned child 场景）。后两拒绝场景仅在受信 no-input shell fixture 中修改环境并 exec 同一个 worker，保持 parent-death/group 与 original FD 继承；未读取/执行用户配置命令。每个实际 child 使用同一 owner 至真实 Exit，epoch0、terminal Missing，未重启超时任务。

成功 console main/bootstrap/扩展循环与 host console 新启动方式只编译，未自动执行；有效配置 exe 测试永久 None。没有查询真实 VT、抓取 evdev、创建 uinput、全局 Hook/SendInput、挂起、driver 或安装行为。新 portable 收尾测试与前序 Windows native 验证仍待补；无新 unsafe/FFI/dependency，C ABI oracle 未重跑。

下一步让原 host Session 持有 reload parent 事务，按实际 ack 代号观察热更新、沿用原 clock/独立 Stop 与监督，在失败时停机而不 rollback/retry；随后接 watcher 和用户 reload 命令。还需 Windows adapter、全部高级规则、图形桌面/GUI/driver、原生 Windows和专用硬件 gate，完整目标继续未完成。见 [验证记录](validation/linux-2026-10-11-reload-entry.json)。
