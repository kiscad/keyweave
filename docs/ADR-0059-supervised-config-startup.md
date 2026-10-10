# ADR-0059：Supervisor 中的配置交付启动事务

ADR-0058 已把 sealed 配置原文件对象交给自有 Job child，但生产 Supervisor 仍只走三句柄启动。增加 `Supervisor::spawn_with_config`：消费已经在 capture 之前准备好的 ConfigArtifact，按固定父/run prefix 与 config handle suffix 创建四句柄 child，复用已有进度 lease/有界 poll/协作停止。该 API 不编译或写文件，不在活跃监督循环中进行同步配置准备；完整 host 的预编译/native output gate/命令接口仍另行实现。

新启动在 callback/policy/run/原 argv/完整五项额外参数和最宽 handle suffix 长度预检之前不创建 pipe/Job/reader。123 extra args 上限与 legacy125区别来自同一128项cap；先检长度与原字符串界限再clone。不改变旧spawn三句柄行为。

失败必须保留已消费的配置与已成功创建的各pipe owners：增加 Config error envelope，内含原 typed cause 与只用于失败清理的 resources。preflight/pipe失败时 config仍在resources；Job调用消费config后，artifact与独立准备错误由原JobSpawnError.owner持有，父pipe仍在envelope中。resources明确close收集所有现有端点/配置的独立结果，callback拒绝不变更；失败端点保留以便retry，调用者保留历史report。底层pair工厂内部失败仍遵循已有有限Drop/poison契约，不假称新envelope拥有未成功返回的内部资源。

成功创建后仅resume一次。resume失败沿用既有Resume error完整Supervisor owner，所有config与父pipe仍可驱动停止/真正观察退出；不能以清理重试允许再resume。新envelope不包裹这个已完整成立的owner，不让错误所有权变得模糊。

实际 Exited 后增加 `close_resources`：在同一准入检查后独立尝试两父pipe与config writer，保留三份结果，不能某一失败跳过其他清理；repeat显式retry不抹调用者先前report。只有实际signaled owned process退出才开放，EOF/receipt/stop请求不算。兼容的close_channels仍只处理pipe。config owner保持到实际Exited及显式close或已确认退出的Drop，未确认Drop沿用ADR-0058保留writer到父process exit。

验收包括preflight与callback零新资源、失败pipe与sealed owner保留、Job主因与父端独立cleanup、resume故障owner直到实际exit、活跃状态禁止close与独立file/pipe失败retry。Windows真实worker仅交付必定无效的UTF8/JSON，从四句柄Supervisor走真实main file admission至实际Config66/零epoch，再分别检查全部cleanup与实际NotFound；无Hook/SendInput。实际入口tests用同一parent进程的互斥串行化所有std Command/native启动，满足不与ambient inheriting spawn重叠的契约。复用check_cooperative的显式--config-transfer模式，用真实guard/原文件bytes和synthetic进度，验证initial/active/standby协作退出与stop_stall/startup_stall超时停止；legacy三句柄模式继续验收。同一Supervisor实际观察exit后明确close全部资源并独立观察NotFound。startup_stall被强制停止并不能证明child已完成file claim/read，其他正常退出/已回fixture epoch场景按具体证据报告；不能当CaptureSession或全局按键恢复证明。

完整host/config无损预编译/native preflight/run-marker/用户控制/详细诊断/热更新、正常capture/hardware与原Linux/D05/GUI/packaging/driver/companion范围继续。没有新增FFI/OS API/协议变更，不以源码/mocks或无输入例程宣称完整移植已完成。
