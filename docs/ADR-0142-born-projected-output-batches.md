# ADR-0142：出生即投影的 Output batch

问题：投影期间新增孩子若先普通准入再排ProjectionDown，原数组可能先交付，产生短暂普通Down或变量Down值泄漏。后代继承需要出生投影。

选择：Output提供prepare_projected_scoped，组合已过滤batch准入与新投影身份。Terminal预备在原批首次实际动作前激活保存来源mask；SourceToken仍来自原预留范围，普通键Down不emit，snapshot owner也被覆盖。变量实际Down后同一步应用保存Up；生成commit也在公开view之前完成同样最终状态。close继续使用成对FIFO marker，不重放原数组，不造新来源。

预备/abort包括batch、scope、来源ID、projection ID/ref、变量/修饰视图和队列；无业务检查在commit后执行。仅非空batch可出生投影，普通准入保持原行为。此为后代继承必要接口，尚未完成Complex producer模板与完整Simple scope；native gate不解除。
