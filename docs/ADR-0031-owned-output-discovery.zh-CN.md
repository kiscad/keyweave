# ADR-0031：从 owned uinput 定位真实 event 节点

## 选择

UinputDevice 已保存 UI_GET_SYSNAME 对应的 canonical input syspath。仅扫描这个目录的直接子项，最多 64 项，要求恰好一个规范 eventN；canonical event 子目录必须仍在同一 input syspath 下。读取至多 33 bytes 的 dev 属性，校验 input major 13 与有界 decimal minor。不要从 eventN 推算 minor，evdev 存在 legacy/dynamic minor 差异。

从 /dev/input/eventN 打开现有的 owned EvdevDevice，核对 actual FD 的 major/minor 与 sysfs 公告一致，再用既有 verify_output_node 检查 canonical parent 与完整虚拟能力。缺少 event 子项、dev 属性或尚未出现的 /dev 节点返回 Pending；权限、能力、身份、歧义和格式错误返回失败，不当作暂未 ready。整个过程只读，不 grab、不发送测试按键，也不扫描其他键盘。

NodeDiscovery 用显式 MonoTime 和有界 policy（默认总期限 2s、重试 20ms；最多 5s、最小间隔 1ms）安排下一次 probe。到期先拒绝、不再访问节点；时钟倒退/探测错误使状态 Failed，完成后不能重复领取。probe 是一次操作，没有内置 sleep 或无限 retry。生产 poll_uinput 在探测前后读取实际单调时钟，结束时已过期则丢弃刚打开的节点；不会交付迟到 Ready。调用者根据 next_wakeup 用既有 ppoll 调度，实际系统调用可能超过请求期限，独立 supervisor 仍负责卡死退出。

## 边界

Ready 仅返回已核验、唯一拥有的输出 evdev node，仍不证明桌面消费者正在监听。禁止把 dev node 出现、udev settle、名称匹配、固定 sleep 或自己打开 evdev 解释为 consumer-ready。下一步完整 bootstrap 需要单独的桌面/控制台 provider。保持 owned uinput 存活直到 Runtime 移交；此发现器不持有、复制其 FD。

测试通过临时文件系统核验 discovery 路径与有界解析，通过 mock probe 核验时钟/重试/过期和错误；没有创建系统 input 节点，不把 fixture 里的普通文件称为真实 evdev。actual open/probe/identity 路径需设备集成验收。

依据：[Linux uinput 文档](https://docs.kernel.org/input/uinput.html)、[evdev_connect 的 parent/设备编号定义](https://github.com/torvalds/linux/blob/master/drivers/input/evdev.c)。本实现不复制内核源码；文档示例 sleep 不是生产就绪屏障。
