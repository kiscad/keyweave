# ADR-0143：Complex 保存父槽的后代投影模板

问题：只投影现有 Output batch 无法覆盖 held 替换或 delayed 等未来孩子，尤其父 main 为空时没有可捕获的 batch。

选择：Complex 对已登记 Reserved/Live 父 ActionSlot 创建有界、actor-local 的模板引用；模板保存设备身份，作用于现存 Output 孩子和引用未关闭期间新生的孩子。多个引用共享该父的一组 Output 投影，最后 close 才排保存来源恢复 marker。即使原 Up、孩子或父完成，关闭凭据仍有效；已完成的来源不复活。热更新不重选原目标。设备代际替换/移除和 Reset 清模板但不回退 ID。

现存孩子批量 begin/末 close 无法同时独占借用同一 Output actor；使用顺序联合 Output 事务。冷检查（时间、due、父身份、模板/引用容量）在任何变更前完成；后续任一 Output 准入失败锁存整个 Complex Failed，禁止继续 poll/admit，必须 Reset；不宣称跨多个孩子撤销原子性，不能将失败解释为成功关闭。单个 Output 的两侧原子性沿用 ADR0140/0142。

出生继承覆盖 main、held、alone、afterUp、delayed invoked/canceled，包括 Reserved 父和空 main。模板本身不改变 Complex 输入匹配视图；Simple consumed scope 的生成/实际 marker 协调仍待接线。native gate 不解除，无原生输入测试。
