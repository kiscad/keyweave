# ADR-0023：evdev 抓取准入事务与启用窗口

状态：采用。关联 D06；不表示捕获 runtime/desktop readiness/watchdog 已完成。

## 核验与问题

依据 [Linux v6.12 evdev.c](https://github.com/torvalds/linux/blob/v6.12/drivers/input/evdev.c) 的 EVIOCGRAB、evdev_release 与 evdev_handle_get_val，以及 [input UAPI](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/input.h)：grab 用 scalar 非零/零参数；关闭唯一 client FD 会释放其 grab。EVIOCGKEY 会清除该 client 待处理的 EV_KEY，而不是无副作用的普通快照查询。

因此启用不能只检查一次 neutral 再无条件 grab，查询失败不能当作空状态，自身输出不能抓取，mixed/unsupported keyboard 不能直接吞掉。也不能在正常捕获期间反复 EVIOCGKEY，或宣称瞬时启用绝不会丢失事件。

## 选择

OutputNodeReady 是借用现存 UinputDevice 的短期证据，只由成功输出 event-node 验证产生：输出仍处于 Ready、evdev canonical input syspath 与 uinput 身份一致，公告 keys 与 DeviceSpec 一致。证据同时借用 spec/identity，不从 name/vendor 猜身份。它证明输出已创建并可从 evdev 节点观察；不证明 compositor/桌面消费者已经打开。后续 runtime 必须提供 consumer readiness/前台启用策略，不能把这个 token 称为桌面确认。

CaptureDevice 独占 CaptureSource。生产源由 readonly EvdevDevice 包装，无 FD 副本或外部可变源访问。准入检查完整 bitmap 范围、keyboard_issues、自身 canonical input path 排除及全部物理 key 的输出 passthrough coverage。后续 runtime 另检查已编译配置的全部输出与平台 capabilities。

arm 事务：检查证据与 capabilities → 查询完整键状态 → 非 neutral 返回等待（不 grab）→ grab → 再查完整键状态 → 只有再次 neutral 才标记 Captured。期间失败/非 neutral 立即回滚 ungrab；回滚 syscall 失败则销毁唯一 source FD，不能继续持有不确定 grab。arm 不做循环或 sleep。read 只在 Captured 开放，release 在未来 capture 停止前调用；失败则 close。Drop 的 production 源尽力 ungrab，再关闭其 owned FD。

post-grab EVIOCGKEY 定义**启用边界**：清除该 client 在正式激活之前的待处理按键；正式激活之后禁止周期性查询。这不是原子 grab+snapshot API。一次完整 tap 如果恰好落在 grab→snapshot 的启用窗口，可能被丢弃；若持键跨越此窗口，post-check 撤销抓取并等待 neutral。没有利用接近的时间戳猜哪个事件应重复输出，也没有把未观察到的 down 编造给引擎。前台 UI/CLI 必须明确显示 arming 与 active，用户在 active 前保持 neutral；稳定静默窗口/consumer 等待仍属于 runtime 工作。真实启用竞态需专用硬件测试，当前不承诺零事件损失。

## 限制与下一步

该组件单设备；不构成多设备原子启用事务。后续 coordinator 需要按全体 neutral 启用，多设备失败撤销已抓取的全部 FD，并控制输出销毁顺序。输出节点 token 只在 arm 期间借用；长期输出存活、lease/recovery、紧急物理组合、broker/supervisor watchdog 必须由 coordinator 落实。不得仅凭 guard API 宣称 engine stall 已恢复。当前不添加会抓取真实键盘的 CLI 命令或系统安装入口。

验证使用 mock 源记录 query/grab/ungrab/close 的顺序和错误，不访问输入设备。本机无 /dev/input、/dev/uinput；成功 grab、close 释放、权限、consumer ready 和启用竞态待真机验收。

## 本次离线检查结果

12 个新增行为测试覆盖 output node 精确匹配、pre/post neutral 顺序、持键等待、查询错误、回滚失败关闭与原错误保留、grab 失败、自身排除/同名不同身份、混合接口/截断 masks/缺失 passthrough、状态 mask 校验、读故障/EOF/oversize、release 与 Drop。完整 workspace 共 283 个通过，fmt/clippy/source verifier 与含 EVIOCGRAB 的 C UAPI probe 通过。Windows CI 和成功设备路径未执行。read_batch fault/EOF 直接 close，不等待失效输出；后续 coordinator 仍负责 output submitted 清理。
