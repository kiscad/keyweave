# ADR-0079：Linux 控制台捕获的分步启动事务

日期：2026-10-10。状态：真实原生组件及 mock 故障验证已实现；实际 main/活动控制循环/host 尚未连接。

## 结果

新增 `bootstrap::ConsoleBootstrap`，把已有 owned uinput、节点发现、CaptureGroup、ConsoleProvider 和 Runtime 连接成单一资源事务。public 接口只能使用原生实现，内部 Platform 注入边界为私有；测试不会通过公开工厂替换父保护或用 fake provider 给生产 caller 发证据。

本切片没有开放 capture-enabled CLI，真实 main 仍只有永久无设备的 none-provider 模式。成功 mock handoff 不是实际键盘可用性验收。

## 冷准备与依赖

调用者必须先在真实 worker 主线程核验父保护、claim reporting/progress 三管道及 sealed 配置、冷编译 Linux ConfigSnapshot/输出 DeviceSpec，并已建立独立父监督。ConsoleBootstrap 不接收普通路径配置、不重读文件、不生成 run identity，也不以 guard 本身代表 progress 端点已准入。

Plan 先拒绝空/超过64输入、零 device id/generation、同 id、相对或重复 lexical 路径；要求 Kernel repeat，validate_config 和 Engine::new 均在设备操作前完成。实际 device/syspath/capability 检查仍由原生 open、节点证明和 CaptureGroup 执行，不把 lexical 路径当原生 identity。输入身份由可信 caller 提供，不能用这些数字鉴权。

`new` 重验 main WorkerParentGuard，打开 ADR-0078 规定的只读选定 VT/provider。没有 open evdev/uinput、没有 grab；但它会查询 VT 元数据，不承诺 syscall 固定期限。

## 事务顺序

每次 step 前重验父保护、检查同一 clock；每次 step 后重新检查时间。

1. 创建唯一 owned uinput，使用冷验证的同一 DeviceSpec。
2. 发现 owned output event 节点；None 保留 Pending。原生 discover 核验设备号和能力；通用事务再核验 OutputNodeReady。保存 Probe 后关闭仅作查询的虚拟 evdev reader，不把它误作 captured input。
3. 每步只打开一个物理输入 client，保存已打开 owners。全部完成后构建 CaptureGroup；重复原生 syspath 在 group 内拒绝。
4. 每次尝试 arm 都重新 observe 控制台，并读取真实当前时间。未知/错误输出 scope/过期证据不会查询物理 neutral 或 grab；None 保留 Pending。有效证据进入已有整组能力、紧急组合、neutral/rollback 事务。WaitingNeutral 保留等待，下一次必须重新观察，而非沿用旧 grant。
5. Captured 后另一步重新 observe。此时任何失去/过期/错误 scope 或 provider error 都撤销已抓取 owners，不能继续等待或进入 Runtime。成功必须再次通过 post-step 时间与 grant 新鲜度检查。
6. Ready 后只能及时消费 into_parts；消费时再次重验 guard、clock、deadline 与 ready scope/期限，才将 group/output 交付已有 Runtime。返回同一 ConsoleProvider、MonotonicClock 与 ConsoleReady，供后续活动循环续期；bootstrap 从未 dispatch/read/write 按键，也不会制造 progress_epoch。

## 期限与退出

期限固定为**首个 step 的查询前时刻 + 10s**，包含后续 handoff 检查；每个采样要求单调、严格 `< deadline`，溢出拒绝。首个 step 之前的冷 Plan/VT/provider 构造不包括在该期限内。整个启动仍需要父监督自己的启动预算，不能用这里的10s延长 parent lease。

最多512个 step，计数即使时钟不动也会耗尽。没有内部 sleep、无限 retry 或后台线程；caller 可在 step 间处理 stop/progress/wait，调度该 Pending 不代表状态成功。原生 create/discover/arm 内含多个有界数量 syscall，期限和操作数限制不证明内核调用及时返回，独立父保护继续必要。

任一 step 错误进入 Closed；Drop 与消费错误同样先关闭 CaptureGroup 和所有 partially opened inputs，再丢查询 node/证据、销毁 output，最后关闭只读 ConsoleProvider。闭态不能重试，重复 step 零设备 IO。Ready 后错误地继续 step 也关闭 capture，不留下无限 held owner。

资源收尾依赖现有 CaptureDevice/UinputDevice RAII；这里的关闭顺序是 ownership 证据，不是完整独立 close/ungrab 错误报告或全局恢复证明。启动阶段未提交虚拟按键，所以不编造 OutputRecoveryConfirmed 或 Runtime TerminationReport；未来 main 必须保留启动错误与 actual exit/report 缺失。已进入 Runtime 后仍使用既有独立终止事实与 KWLT。

## 验证与剩余连接

新增9个 mock 行为测试：观察在 grab 前后、真实 Runtime handoff 但 epoch0/零dispatch、未注册节点/持键等待、缺失/错误 scope/过期消费者的零query/grab、post-grab丢失/查询错误、partial-open及node identity错误、guard/create错误、严格期限/overflow/倒退/512预算、handoff过期及captured状态Drop、冷identity拒绝、查询后时钟与late-Ready错误。trace 验证所有输入关闭先于输出、provider 最后关闭；未访问真实 VT/evdev/uinput。

Linux Rust 1.97.1 的完整 workspace all-targets 817 tests、Clippy -D warnings、fmt、60-source verifier 通过。没有新增 FFI/unsafe/UAPI，C ABI oracle 不变、本轮未重跑。初步8个组件测试通过；初次 Clippy 报泛型返回类型复杂度，已改为有名字的私有 Handoff，没有关闭生产 lint。[验证记录](validation/linux-2026-10-10-console-bootstrap.json)

Windows 原生验证本轮未执行；本模块仅 Linux cfg，此前 ADR-0073/0074/0077 待补原生检查仍保留。

下一步先接 worker 的 bootstrap/control 状态循环：在 Pending 期间处理 Stop/监督与真实有界 wait，在交付 Runtime 前重验消费者，活动中定时续期/失效先停止capture，所有 provider/protocol/Runtime 用同一 clock；不能将 bootstrap metadata/step 次数当 Runtime poll 进度。之后开放明确 console 参数并连接 sealed-config main，仍保留 none 的永久无设备 exe tests。再实现 Linux host 的冷准备、独立监督、KWLT+actual Exit 报告和专用系统真实输入验收。

图形桌面就绪、睡眠/会话撤销、多平台高级规则、热插拔/add、LED、热配置、GUI、打包、Rust driver 与 companion 审计等完整目标继续未完成。
