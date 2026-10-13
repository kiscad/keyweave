# ADR-0149：Linux 冷准备、捕获组和 bootstrap 固定执行模式

显式 ExecutionMode（Legacy/StagedKeyboard）从 PreparedConfig 保存，经 bootstrap Plan 与 CaptureGroup 传到 Runtime；已有工厂默认 Legacy。冷验证按同一模式校验 immutable snapshot，output spec/repeat/neutral/consumer-ready/设备集合所有原准入不放宽。模式在会话生命周期内不可切换。

reload 的 LinuxStaged 目标按 staged 模式冷编译并保存 candidate 模式，sealed actor 必须匹配模式才允许 apply/Applied；仅 platform 相同不够。默认 Linux/Windows 目标保原行为，Engine 不能接 staged candidate。封存原配置 bytes 不重序列化，模式是调用方持有的元数据，不能宣称现有跨进程 artifact 自动交付它。

本阶段将工厂/准备贯通，尚未新增 native worker/host CLI 的 staged 选择参数；真实捕获及平台验证继续。

实际 JSONC 发现编译器未消费 hold_down_milliseconds，且 IR 写死 0。补整数 0..86400000 毫秒、checked ns 转换，非法数值按精确 action-option pointer 拒绝，未知字段仍拒绝；Legacy Engine 非零 timing gate 不变。这是 staged 冷准备能从真实配置读取 duration 的必要解析，非只改测试 IR。

7 个新测试、工作区 1374 用例/Clippy/fmt/source60 通过。mock bootstrap handoff 没有读写输入/输出 dispatch，捕获组确认同模式 Runtime，默认 Legacy 同一 timed 配置仍拒绝。Cross-process artifact/CLI 尚未携带模式，后续必须将固定启动参数与 worker 独立重验一起接入；默认 native 入口保持原 mode。
