# ADR-0030：多设备捕获准入事务

## 选择

CaptureGroup 独占最多 64 个输入和不可变配置。构造时核对非空/容量、唯一 device ID 与 canonical syspath、Standby 阶段和 Engine 配置可执行性；try_arm 在任何 state query/grab 前对全部设备验证 OutputNodeReady、完整能力、own-output 排除、passthrough 覆盖、Kernel repeat、配置输出覆盖及物理紧急组合可达性。

之后先查询所有输入 neutral，全部通过才依次调用既有单设备 pre-query→grab→post-query guard；全部抓取之后再查询全部输入 neutral。任何按住返回 WaitingNeutral，并撤销所有已完成的 grab；失败保留原设备/错误，尝试撤销全部已捕获输入，收集有限 rollback 错误，再 close 全部输入。rollback 失败不可继续使用组。成功进入 Armed，只允许一次消费移交给 Runtime，不允许再次 query/arm。

组的 Drop/关闭始终关闭全部输入；into_runtime 在未 Armed 时先 close 输入再 drop backend，成功时 Runtime 再核验实际 backend 身份与配置，保持原有输入先于输出释放的顺序。组不复制/导出输入 FD，不在准入阶段交付引擎事件或虚拟输出。组不持有 output owner；调用者必须令同一 owned output 从准入前一直存活到 Runtime 移交，不能将短期 OutputNodeReady 当成跨调用 lease。

## 边界与替代

OutputNodeReady 仍只证明 owned output 的 event node 身份/能力，不证明桌面消费者打开。调用者必须先取得真实 consumer-ready；本组件不是可部署 bootstrap，不提供以 bool 或固定 sleep 代替 ready 的旁路。后续生产入口接入 WorkerStartup、输出 discovery/consumer gate 和组准入。

evdev 没有跨设备原子 grab+snapshot。逐个 query 的 neutral 是离散采样，不是全组同一时刻快照；最后一个查询后仍可能立即发生输入。EVIOCGKEY 会 flush client 已排队按键，因此整个 arming 窗口可能丢完整 tap，或形成物理应用见到 up 未见到 down 的转接边沿。增加最终检查只能拒绝观察到的 held，不能承诺无损切换。查询仅允许 Standby/arming 边界，移交 Runtime 后禁止周期 query。

单设备独立启用会留下部分设备抓住而后续失败的状态；只依赖构造 Runtime 失败后 Drop 会使配置/能力错误先于检查抓取。这里先做全组 preflight，并对部分成功显式 rollback。为追求无损而无限等待不存在的内核原子操作不作为实现。

## 验证

mock 故障注入验证 preflight 全部通过才 query/grab、任意设备按住不留下部分捕获、最终 neutral 失败撤销全部、grab/query/rollback 故障及主错误保存、重复身份与容量、Drop 和 Runtime 移交。真实多键盘切换窗口、抓取/释放与桌面 ready 仍需专用机器验证；不把 mock 作为硬件成功证据。
