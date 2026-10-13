# ADR-0164：组合键空主动作的共享准入

合法 simultaneous 匹配的主动作可能为空，仍需消费并保存全部原 Down，直到成员原 Up 和后续分支结束。不能用 identity 输出或零槽孩子替代。

BindingArena 的 SharedConsumedDown 独占持有所有原 receipts/payload、主成员的空目的凭据以及共享保留候选。完整预检来源、时间、成员唯一性、容量和目的身份；拒绝返回所有输入，abort/drop 不激活、不占共享 ID。commit 一次激活全部成员并安装一个共享保留，不创建孩子、输出槽或额外来源。

OutputStageActor 的空共享准入联合 Generated/Terminal 空计划 guards 与 SharedConsumedDown。任一拒绝不提交成员、变量或输出；commit 消费合法目的 batch ID，保存主动作过滤结果，无 recipe/完成报告。GroupBindingOwner 以 main=None 构造，主工作已结束，但全部原 Up、外层分支和其他孩子/保留的完成屏障继续有效。

接入证明使用实际仲裁结果、实际 ProducerDecision、共享空准入和拥有者：空动作 any/all 吞掉原键，成员逐一原 Up 后才释放组；空 main 后仍可从保存主规则生成 after-up 子计划，须子工作完成才回收成员。生产调度器和配置 KW-SIM gate 在所有分支、回放、投影与控制接入前仍关闭。
