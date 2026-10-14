# ADR-0183：有界 owned X11 探针任务与结果验证

隔离executable不能用阻塞Command.output接到活动worker。选择ProbeJob独占一个trusted显式image child，stdin/stderr null、私有stdout pipe预先nonblocking，argv结构化目标；parent-death hook保留，creator线程须活至真实exit。一次poll最多读4096 bytes、总196608，单deadline<=500ms；不创建第二个任务、不从结果推断capture-ready。

clock倒退/超时/取消/输出超量/read错误请求一次kill并丢结果，kill不是exit；保留同child直到try_wait确证。EOF与code0均必需才解析；不完整、错误exit、重复/未知JSON字段、过量/duplicate id、不绑定display/node/设备号等均拒绝。结果只表示XI metadata，空devices合法unknown；后续consumer gate还需session前后观察/输出node和能力。

Drop仅best-effort kill与<=20ms观察，不宣称完整回收；正式调用者保留poll至Exited。可信探针不fork子进程，非trusted/任意PID/通用shell不是此接口；命令和helper路径认证由host定位，canonical regular+execute位不等同签名。没有ambient FD allowlist声明，input/output FDs仍必须CLOEXEC。

测试用独占临时helper fixtures，无显示/设备操作；验证实际进程stdout、deadline、kill后exit、invalid/cancel和一次取结果。图形消费者、SDK/live Xorg验证、mixed host/worker继续，不把metadata成功作为mapping成功。
