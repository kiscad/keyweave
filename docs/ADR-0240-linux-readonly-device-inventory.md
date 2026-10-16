# ADR-0240：Linux 有界只读 evdev 元数据列表

新增独立 inspect 路径和 list-linux-devices CLI。列举固定 /dev/input 下 canonical eventN，最多1024目录项/256候选/2秒观察预算，数字排序与去重。目录失败、限额、clock异常、probe错误分别保留；不把部分列表称为完整物理设备宇宙，不按name猜keyboard类型，也不自动选中capture。

EvdevDevice.open 仍沿原canonical别名与monotonic clock行为。inspect复用同一版本/char major/FD-ID-sysfs前后核验，但直接no-follow打开event路径，不配置clock，不read事件/EVIOCGKEY，不grab/uinput。元数据复制后释放FD owner。libudev trusted类型查询仍可Unknown，不放宽root-trust；capability issues只是支持表诊断，不是neutral/consumer/lease/output readiness。

扫描独立于Runtime inventory，不更新conditions上下文或发DeviceAdded。结果schema明确scope canonical_evdev_nodes、scan_complete与probes_complete，unknown types保持null；name输出UTF8替换标记、JSON转义，不输出raw事件/pressed状态。每错误保留kind/errno，不因单节点无权限丢掉其他结果。时间只在同步操作之间检查，不声称kernel/FS/libudev调用硬实时可中断。

该入口仅显式CLI调用；GUI异步列表与选取/热插拔继续后续接入。默认测试用synthetic names/probe/clock，不访问设备。真实设备、Windows原生、硬件/Wayland与完整目标继续。

验证：新增5项（4扫描/metadata/时钟与限额，1CLI参数），Linux workspace2015通过/8已有native默认ignored；格式/Clippy/源码62/Linux ABI/X11 SDK/regex oracle通过。实际 list-linux-devices 输出schema1不完整快照、exit1、input_directory NotFound/errno2、0entries，环境无/dev/input，未触达物理FD/ioctl，不能称硬件探测通过。见 [验证记录](validation/linux-2026-10-16-device-inventory.json)。

预算在同步操作之间观察；目录冷打开/单次syscall/provider阻塞没有硬实时保证。下一步GUI以独立单任务调用scan，显示source/age、partial与type Unknown，并在显式选择后仅生成配置selector草稿，不当运行时准入或全量conditions inventory；实际设备列表/热插拔/原生硬件验收继续。
