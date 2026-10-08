# ADR-0025：不可覆盖的物理紧急退出

状态：采用。关联 D06/ADR-0024。

## 决策

默认退出组合为同一已捕获物理设备上的 Ctrl + Alt + Shift + F12，连续保持 1 秒（`>=` 阈值）。左右 Ctrl/Alt/Shift 均可；每组有任一侧仍按下就保持。组合不允许拼接不同键盘，防止多人/多设备偶然共同触发。普通其他键不改变组合；repeat 和重复 down 不重新开始计时。最后一个必需键松开撤销等待；完整组合再次形成才重新计时。

独立 EmergencyChord 只保存 7 bits、单调时钟和一个可撤销期限，不记录普通按键。原始 EV_KEY 在完整帧验证后、normalizer/Engine 匹配之前交付检测器，规则吞掉/重映射修饰键和 F12 无法覆盖它。MSC 不能伪造按键，native repeat 不能凭空建立 down。每设备一个检测器，生命周期与捕获设备一致。

Runtime 启动要求至少一个参与捕获的接口公告 F12，以及 Ctrl/Alt/Shift 每组至少一侧。没有可达物理组合时明确拒绝启动；复合 consumer 接口可以与可达的标准键盘共同启用。未来其他组合/外部控制路径需要明确配置与准入，不暗中禁用默认退出能力。

Runtime next_wakeup 包含 emergency deadline。到期时先于普通读取、引擎 deadline 和映射输出触发停止，先关闭所有捕获 FD、撤销引擎 timers/macros，再执行有限 submitted 恢复。退出原因独立为 Emergency；不输出按键日志。Detector reset 清状态、保持时钟单调。超过计时范围明确报错并停止捕获。

当前使用 Runtime 的显式交付时钟，和已有输入/held/delayed 行为一致。release 在 `< 1s` 被交付会取消；在 `>= 1s` 的 poll 已到期，退出优先。若 OS 队列严重积压，尚未读取的早期 release 无法修正已经观察到的连续按住；不能以离线时钟测试声称实际物理时长精确。OS 循环需及时读取，延迟/积压与跨设备 round-robin 边界需硬件验证。

## 边界

这条通道独立于映射，但仍运行在捕获线程。卡死线程不能自行识别组合；supervisor/watchdog 必须另外实现并监控实际事件循环进度。实现紧急组合不等于 engine-stall 恢复已完成。尚未添加默认系统捕获命令，没有实际设备测试。

## 本次验证

7 个 detector 测试覆盖 `>=` 边界、one-shot、缺少类别/orphan repeat、全部左右组合和重叠、重复/其他键不刷新、释放重新计时、clock/overflow 原子拒绝，以及数值与 HID table/准入一致性。6 个 Runtime 测试覆盖吞掉全部组合键的规则、右侧组合取消/重新计时、跨设备不拼接、同刻 held/queued release 优先级、半帧不启用/溢出停止，以及无可达组合的启动拒绝。workspace 共 312 个通过，fmt/clippy/60-source verifier 通过；没有 hardware/Windows 执行。
