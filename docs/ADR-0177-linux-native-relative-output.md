# ADR-0177：uinput 相对能力与共用写出状态机

混合 pump 需要真实后端声明已创建的 REL axes，而不只是 mock。选择 DeviceSpec.with_relative_axes 显式冷扩展（默认仍无 relative），uinput 创建先设置 EV_REL 与各 UI_SET_RELBIT，再 setup/create。evdev probe 查询完整16位 relative mask；OutputNodeReady 精确核验指定轴/事件，不把声明等同原生事实。physical keyboard admission 保持拒绝相对设备，Runtime 尚未捕获 pointer。

提取 OutputChannel<W> 持 writer/layout/spec/state；UinputDevice 独占 OutputChannel<File>，身份/RAII/ioctl/反馈沿原 File。真实 emit、emit_relative、release_submitted 和 flush_report 与 mock 使用同一状态机。相对写只允许 Ready，能力拒绝在 writer 前；short 到 ReportPending、unknown 到 Quarantined，报告完成到 Recovery，仅已知 key Up 可恢复。不为相对事件造 owner/recovery packet。

Probe metadata 必须包含 relative mask，既有 tests 补空16位 mask；EV_REL query/header常量/RELBIT ioctl 由本机 C oracle 核验，无实际设备操作。创建失败仍close/RAII销毁，capture gate 与 consumer授权不由 OutputChannel 构造证明。默认工作流和文本消费者保持原范围，pointer capture/消费授权将在后续明确接入。

无新unsafe边界/依赖；private ioctl whitelist 明确新增 EV_REL/支持REL codes，沿原已审计scalar调用。来源同 ADR-0175 的 Linux 官方 input/uinput 文档，原生与portable证据分别记录。
