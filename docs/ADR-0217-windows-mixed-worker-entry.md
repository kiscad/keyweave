# ADR-0217：Windows mixed 私有启动参数与实际入口

严格Windows argv增加可选 --capture-scope staged-mixed，位于execution-mode之后、owned config handle之前；mixed强制staged-keyboard且必须继承config handle，禁止mutable路径fallback。既有Keyboard入口/argv保持默认，最大参数15，仍有界读取16并拒绝重复/未知/乱序。builder新增with_scope，保OsString/path/tag规范，由Supervisor事务追加真实继承handle。

真实main先claim原WorkerStartup，消费同owned reader按mode/scope冷编；reader close仍独立检查。mixed明确进入MixedWorker，双Hook/recovery/report原owner保留至退出。TerminalFacts.with_mouse_unhook按原keyboard结果 AND mouse结果合取，无新增wire/位；任一失败均保UnhookUnconfirmed，其他所有独立fault/cleanup位不丢失。mixed报告仍保两API错误，退出mask不承诺全局恢复。

此提交完成worker exe选择，不自动更改host/CLI默认或启用设备。Windows-only main执行/原生构建仍待验收，Linux参数/编译/纯terminal测试不能替代Windows main/Job/Hook/实际输入证明。
