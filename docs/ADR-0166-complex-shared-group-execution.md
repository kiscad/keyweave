# ADR-0166：Complex actor 的共享组执行路径

将实际 ChordArbiter 的 selection/ChordMatch 接入现有 ComplexStageActor，复用其输入表、BindingArena、Output actor、普通分支、孩子表及退休路径。组只增加一个 primary owner 和 member→primary 索引，不创建另一个引擎或虚假物理按键。

匹配准入完整预检每个成员的原输入计划/来源/旧 Arc、原 Up flags、保存规则/完整 simultaneous 定义、生成时钟和分支 deadline，然后联合共享 Down 与实际 main Output guards。空 main 走共享 consumed guard。原历史 at 保留，实际 Down/initial Up marker 用当前生成时间；lookahead 原 Up 仍在保存成员上真实提交。所有共享 handles 保存至 owner 或失败隔离表，不在错误/Drop 时丢失。

全部组成员的实际 Up 路由到唯一 owner；any/all 主释放只触发一次普通 Alone/AfterUp，全 Up 另触发 group-after-up。孩子完成先报告给组 owner，再移除实际孩子；成员只在所有输出、延迟分支和 owner 屏障结束后按原输入表记账退休。Held 替换同步更新 owner 的保存主孩子及所有成员 repeat 查询。Reset 先输出 reset，再显式解开 owner/失败隔离保留，最后取消输入树；这些是纯逻辑事实，不是 native accepted ledger 恢复。

默认构造/配置 KW-SIM gate 保留。当前匹配准入是外层有界仲裁器的显式交接接口；本阶段在同一实际 actor 的内部测试构造中验证执行，不宣称已经接通 Simple 的 frontier/待回放队列或原生会话。自动等待窗口/条件重新检查、普通规则回放、跨成员设备撤销、投影继承、全局来源预算和 native repeat 尚须统一调度验收，生产 gate 不能提前放开。

定向测试发现 initial lookahead Up 不能越过同刻尚未执行的主输出 Down。实际原 Up marker 在准入时提交，但 group initial facts 作为有界 pending actor work 排在 due Output 后、Held/Delayed timer 前；每次 poll 保留所有 OutputIntent，不内部 drain/discard，也不重放原 Down。Reset 直接取消 pending initial work，不触发普通或全组抬键分支。

普通组 Alone/AfterUp 的条件身份使用实际释放输出的成员，输出生命周期仍归 primary。Alone 的严格时间跨度用保存的原 Down/Up event 时间，延后生成用实际 poll clock，避免 lookahead 延迟或晚 poll 缩短/延长用户按键时间。任何成员原 Up 都取消 Held，包括 all 模式尚未释放主输出的情形。组外原 receipts 以完整来源判断移交 replay，不因槽编号相同而丢弃其他来源。

后续接入顺序：

1. Complex 持有首个原 receipt 的仲裁器和有界 replay 队列。Simple 必须提供真实未交付事件 frontier；不能以一个晚 poll clock 或暂时空队列冒充输入 EOF，让阈值内的旧输入被提前超时。保持阈值边界包含和同刻控制优先，64-work Yield 不能变成错误。
2. 新 Down 的旧 Held/Delayed 打断先于仲裁 fresh view 和最终选择。当前 admit_matched_group 在仍有旧 delayed 时冷拒绝，不负责在已冻结 selection 后执行旧 canceled 分支。Ordinary/Pass 回放仅对首输入使用保存结果，其他原输入按顺序重新进入仲裁，实际准入用当前时钟，历史时间不改。
3. 任一成员设备失效必须撤销整组实际输出和候选，再解除共享 hold。未失效设备的未抬起成员保留已取消绑定等原 Up，禁止触发 Alone/AfterUp，也不能删除其同批次其他槽。SharedRetention 阻止部分 cancel_subtree 的限制不得绕过。
4. 组任一成员的 descendant projection 必须覆盖共享主输出和后续孩子；核对引用、来源预算和 repeat。完成上述 actor 与 Simple/Physical 联合回放验收后才更新编译/runtime gate，Windows/Linux 原生与硬件验收仍分别进行。
