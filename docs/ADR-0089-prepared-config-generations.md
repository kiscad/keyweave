# ADR-0089：冷配置准备的显式代号与下一代候选

日期：2026-10-10。状态：冷准备 API 已实现；原生热配置交付待连接。

## 问题与决策

Engine、Linux Runtime 与 Windows SessionActor 已有不可变快照激活接口，且保留 held binding 原 Arc。但 worker 的 PreparedConfig 始终用 generation1 编译；直接准备第二个配置不能作为下一代候选。不能靠热路径修改公开 IR 字段或把最新 prepared candidate 当作已成功激活。

两平台 PreparedConfig 增加 `read_for_generation`、`from_bytes_for_generation` 与 `read_next`。旧 read/from_bytes 与默认 startup 保持generation1。共用 compiler helper 接受显式非零代号，原 callback-context、大小、UTF8、loss、能力、output/engine preflight 校验保持。generation只是控制元数据，不是 run 身份或授权。

`next_config_generation` 要求 observed current>0，以 checked_add 计算下一代。zero/currentMAX 立即返回 LoadError::Generation，read_next/read_for_generation在读取输入之前拒绝无效值；不使用全局计数器、时间或回绕 fallback。显式最后一个合法 u64MAX 仍可冷编译，但下一次更新需明确处理耗尽，不能沿用旧持键自动重开一个 run。

## 激活与字节边界

调用方必须使用 actor 已观察的 active generation。候选准备成功、排队或失败都不自动推进该值；同一个 observed base 的多个候选会产生同代号。actor 在切换时仍拒绝 stale/non-increasing generation，并校验实际 output spec、变量历史等动态资源；准备不能替代 commit ack。失败保持活动配置和绑定。

旧 down/repeat/up 使用保存的 binding 与 Arc；候选激活后的新 down 使用新快照。精确原始 JSONC bytes 保留，没有重写 generation 到用户文档。seal只交付 raw bytes，不带 generation；默认 worker startup重新编译仍是1。未来 reload IPC 必须独立交付 run/request/target generation 与候选，并返回 actor实际接受的代号，不能仅复用启动 sealed image 后称为热更新。

所有 read/parse/regex/compile 都是冷工作，在callback/正常输入poll之外执行。当前没有加入文件监听或 callback中的文件读取。

## 验证与后续

[验证记录](validation/linux-2026-10-10-config-generations.json) 中四个 portable tests 同时执行两平台 cold factory与确定性Engine：旧 held repeat/up仍输出原键，新 down使用新键；失效配置不推进代号；同base候选的stale激活拒绝并保持状态；zero/MAX不调用reader；最大合法代号/exactbytes不回绕。没有新文件、worker、捕获/输出/driver操作。

完整 Linux tests、Clippy、fmt、60来源检查通过，无新native ABI/unsafe/dependency，C oracle未重跑。四个新portable tests、默认冷路径改动及之前的Windows待补native checks尚未在Windows运行。

后续实现有界单在途 reload 请求、worker外callback的候选准备、actor控制优先级/commit ack、父端异步文件监听与失败诊断。Stop、session失效与父lease保持更高优先级；reload不能伪造capture progress。完整跨平台任务仍未完成。
