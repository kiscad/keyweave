# ADR-0202：relative reload 的 mode/scope 绑定

新增 Target.LinuxRelative，使用既有 explicit StagedRelative 冷准备；Candidate携固定Linux mode和scope。deliver在actor.apply前检查platform、generation、mode、scope，Runtime以实际execution_mode/relative_enabled回答；Keyboard与relative候选双向拒绝，不因同为staged而放行。

Target.linux_prepared直接由immutable PreparedConfig 的spec/mode/scope选Target。默认Linux/LinuxStaged均携Keyboard scope，Windows无Linux scope，sealed Actor默认仅接受Keyboard。后续host/session reload lane必须使用该helper；当前X11完整session尚未接reload extension。

测试运行实际compiler thread、Receiver协议和Engine generation mutation，scope metadata用私有sealed test actor模型（不是实际Linux Runtime）。实际Runtime的mode/scope accessor已有构造测试，此次adapter只编译。没有设备/捕获/consumer I/O。
