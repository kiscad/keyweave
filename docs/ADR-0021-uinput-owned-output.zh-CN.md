# ADR-0021：uinput 输出设备、短写与不确定接受量

状态：采用。关联 D06；真实抓取、runtime 和硬件验收未完成。

## 问题与来源

输出 API 必须区分引擎 intent、内核接受的边沿以及帧边界，设备名称不能证明来源。依据 Linux v6.12 的公开 UAPI 与驱动行为核验：

- [uinput UAPI](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/uinput.h)：setup、version、sysname、bit 与生命周期 requests。
- [uinput 实现](https://github.com/torvalds/linux/blob/v6.12/drivers/input/misc/uinput.c)：scalar bit 参数、sysname 返回长度及 write 的错误行为。
- [内核 uinput 文档](https://docs.kernel.org/input/uinput.html)：创建后消费者枚举的时序。

本项目独立编写 Rust 封装，不复制内核驱动实现。请求编码与布局另由本机头文件 C probe 检查；C probe 只检查 ABI，不调用设备。

## 选择

`output::DeviceSpec` 限制名称为 1–79 bytes、无 NUL，native key codes 必须属于当前 154 个 canonical 输出码；重复码合并，输入列表也有界。setup 是独立编码的 92 bytes：BUS_VIRTUAL、无第三方 vendor/product 身份、version 1、零 force-feedback 配额。

`uinput::UinputDevice` 在 Linux 上拥有一个不可复制/导出的非阻塞 read/write FD。仅接受明确的 uinput 节点，查询 protocol 5，设置 EV_KEY 与所需 keys。Kernel repeat 启用 EV_REP 并拒绝外部 repeat edges；External repeat 不启用 EV_REP。可选 LED 反馈只公布 Num/Caps/Scroll，不代表物理 LED 同步已经实现。

私有 ioctl.rs 是唯一 unsafe 边界。UI_SET_*BIT 使用 scalar 参数，UI_DEV_SETUP、UI_GET_VERSION、UI_GET_SYSNAME 使用对应的初始化 buffer。CREATE 后立即建立 RAII owner；后续身份查询失败会销毁已创建的设备。Drop best-effort DESTROY，再由唯一 FD close 收尾。

读取内核 sysname，严格限定 `input` 加十进制数字；保存 canonical `/sys/devices/virtual/input/inputN` 路径。后续 runtime 必须与 evdev probe 的 canonical input syspath 比较，排除自身。名称或 vendor/product 不能替代路径比较。`OutputState::Ready` 只表示可写状态，不是物理 capture 准入凭证；仍需消费者/节点 ready、own exclusion、neutral 与 watchdog 等独立 gates。

每次输出有界为 256 个 EV_KEY 加一个 SYN_REPORT；整批校验后只执行一次 write，不使用 write_all。正返回的整记录前缀可以确认其 accepted 边沿。接受全部 KEY 而缺少 REPORT 仍为短写，设备进入 ReportPending，普通输出暂停。显式 flush report 成功后只允许调用 release_submitted 发送 runtime 账本中的 up；恢复后设备保持 Recovery，本版要求销毁并重新创建，不能自动恢复抓取。

errno 或非整记录返回属于 UnknownAcceptance：内核错误可能发生在注入部分记录之后，不能伪造 accepted=0。设备进入 Quarantined，拒绝普通/恢复写入。调用者必须释放物理 grab 并销毁输出 FD；本封装不会猜测要补多少事件。`InvalidFrame` 是写入前拒绝，不能混同未知接受量。

## 替代方案与限制

不选择无条件重试整批：它会重复已接受的事件。暂不实现 legacy uinput_user_dev fallback；不猜测未来协议版本。另一个可行方案是成熟 Rust uinput 库，但当前离线缓存没有合适依赖，仍必须核验其短写与 errno 行为。

当前 read_feedback 提供有界原始反馈，session/runtime 才负责 LED 状态来源与物理同步。本组件没有 grab、热插拔、事件线程、desktop readiness 等待，也没有配置到 DeviceSpec 的实际准入编译。Windows 不导出 uinput FD API，但 portable output codec/config/write 故障测试可在 Windows 编译执行。

## 验证

八个新增离线行为测试覆盖完整帧、全部正短前缀、errno/非整记录、不写空/非法帧、显式 REPORT flush、spec/setup 限额、repeat 单一来源，以及 `/dev/null` 路径拒绝。后者在创建前失败，不访问真实输入或创建虚拟键盘。

本机无 `/dev/uinput`、`/dev/input`；成功 CREATE/销毁、自己的 sysfs 身份、consumer readiness、真实短写/权限失败、LED 与 kernel repeat 均待真机验收。mock 与 ABI 检查不能替代这些验收。
