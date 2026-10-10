# ADR-0073：Windows 会话执行者的上下文交付入口

日期：2026-10-10。状态：执行者入口已实现；原生 provider 与 CaptureSession 接入待实现。

## 问题与决定

配置编译器与共享 Engine 已支持应用和输入源条件，但 Windows SessionActor 只能交付按键与计时器。即使控制面取得可信上下文，也没有可以遵循执行者顺序和故障策略的交付入口，条件一直保持 Unknown。

新增 `SessionActor::context_changed(ContextSnapshot, MonoTime)`，由同线程的可信会话控制面在 Hook callback 外调用。该入口不查询 OS、不读取配置、不建立线程、不激活捕获，也不宣称已实现前台应用监测。

## 顺序、预算与错误

先检查 Active 和单调时间，再完成已到期的输出及 Engine 任务；它们使用更新前的上下文。随后交付 ContextChanged，最后检查新产生的到期工作。与已有 key/tick 一致，整个调用共享 MAX_WORK=64 的操作预算，而不是给各阶段独立预算。

ContextChanged 通过原 dispatch 进入共享 Engine，使用统一递增序列。Engine 校验 revision、provider、有效期、字段长度与设备库存；无效快照不发布、不消耗成功事件序列，执行者进入 StopRequired。到期任务可能已经执行，因此不能把失败更新描述为整个调用没有副作用。调用者必须停止 Hook，再进入已有有限输出恢复路径。

快照不重新匹配已有 binding。旧来源 up 继续释放其原输出，新的 down 使用当前快照。过期或缺失字段仍按三值条件处理：Unknown 不使 unless 成真。该方法不会延长有效期、创建输出 owner 或改变 config generation。

## Provider 接入约束与剩余工作

未来 CaptureSession 的控制循环在 callback 外采样，以 actor 的时钟域产生 observed_at/valid_until；应用、输入源和设备字段须由一个合并控制面共同维护 revision，不能让单个 provider 更新覆盖其他字段。采样失败应显式发布未知值，不能延用失效的上次应用路径。

Windows 应用 provider 要核验前台窗口及进程身份的采样竞态，使用进程可执行文件路径；不能将进程名称、窗口标题或路径伪造为 macOS bundle_identifier。Hook 没有可信每事件设备身份，不能以全局库存补造设备匹配。

当前尚未调用原生应用 API，也未把入口接到 CaptureSession/worker 控制循环。因此本 ADR 不解除条件 provider、上下文热更新、真实桌面验收或其他未完成任务的 gate。

## 验证

新增四项 SessionActor mock 集成测试，覆盖完整按键—Engine—OutputPump 路径：

- 应用变更后旧 binding 正确松开，新的 down 采用新上下文；上下文与按键共享序列。
- 严格有效期边界及缺失应用不让 unless 成真。
- 与 held deadline 同刻更新时，先执行使用旧上下文的 held 分支，再更新；原 up 正确释放替代输出。
- 无效 revision 不发布新快照，保留已提交输出至 unhook-first 恢复。

Linux 固定 Rust 1.97.1 的全套 779 tests、all-targets Clippy 与 fmt 通过，来源核验通过。Windows 原生双架构命令被自动审批的额度错误阻止，未执行；旧原生编译报告不能替代这次修改的验证。见 [本次记录](validation/windows-2026-10-10-context-ingress.json)。无新增 FFI/unsafe，不运行 Hook、SendInput 或驱动操作。
