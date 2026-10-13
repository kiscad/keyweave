# ADR-0171：Physical 组合键来源预算与完整模型入口

静态来源预算必须把 simultaneous 的任一成员当作潜在 primary，而不是只查 mapping.from 原型。每个成员成本覆盖完整 main、Held/Alone/普通 after-up/Delayed 和 simultaneous_options.after-up，以及既有独立快照修饰来源。Simple 的每个目的键和额外修饰键沿同一表展开；同键候选取最大，多个后代保守累加，不递归重映射 Output。

共享 retention 和 projection handle 不分配 SourceToken，不伪加来源；来源 lease 保留到真实上游成员完成或设备撤销。给每个可能 primary 全额保守预留，重复预留不等于实际 owner count，但不会在跨设备匹配后借用已退休来源。旧 Root 的 lease 保留原成本，reload 只改变新 Root 的静态表。

PhysicalStageController.new_with_chord_model 使用同一 controller、history、source-domain 及整个 Simple/Complex 模型，并沿用 16384 上限和准入前容量检查。模型的 Output allocator 仍严格限制在低位域，Physical/pointer origin 保持高位域；Reset 不重用身份。默认 Native 工厂、compiler KW-SIM/runtime gate 保持，模型验收后再统一原生模式。
