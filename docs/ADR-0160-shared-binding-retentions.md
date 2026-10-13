# ADR-0160：多参与者绑定保留与取消顺序

组合结果可能在首成员 Up 后仍因 all、延迟数组或作用域保持输出；不能只用第一参与者的单父子边保证其余输入来源存活。BindingArena 增加独立共享保留：同阶段的 Reserved/Live 槽共同登记，不创建输出、SourceToken 或虚构 Up。

独占 prepare guard 在完整成员/容量/阶段校验后 commit；abort/drop 不占 ID/引用。handle 不可 Clone，含 arena 私有实例标记与单调 ID，跨 arena 即使数值 ID 相同也不能释放。group 最多 4096、成员引用共 16384、每组最多 32；重叠保留计独立引用，最后解除前 take/retire/cancel 均不得移走参与者。

cancel_subtree 在任何树节点移除前检查所有后代保留，避免孩子已删除才在父节点失败。提供只读 blocker keys 和原成员查询，拥有 group handle 的 actor 先取消共享实际输出/未来任务与作用域、解除保留，再清目标子树；其他设备成员可随后按其原 Up/实际孩子状态退休。handle 解除只表示元数据保留结束，不证明输出恢复或 OS 接受。

本层不自动在 Drop 解除保留，不自动取消跨设备输出，不声称与多 actor 事务整图原子；接入者须保有 handle，失败走既有 Failed/reset 控制。组合匹配后的所有参与者激活、输出联合准入、branches/projections/replay/repeat 仍须接到 Complex actor，KW-SIM/runtime gate 继续保留。
