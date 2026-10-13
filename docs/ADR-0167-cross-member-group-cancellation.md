# ADR-0167：任一成员设备失效时撤销共享组

Complex 的设备控制先用 Output 的双历史检查核验 generation/时间，不先改变组。任一保存成员的 device 被移除或新 generation 替代时，整组主输出、旧 Held 孩子和后续分支孩子都用实际 Output.cancel_batch 撤销；保留所有返回 OutputIntent，再取消对应实际 arena 孩子，取消 Held/Delayed/Alone 和尚未应用的初始 Up。

组输出撤销后才消费 owner 的共享 handle 并显式释放 retention。失败保留 owner 或隔离 handle，由原 Failed/Reset 流程处理，不在 Drop 清 hold。成员索引解开后，未失效设备的尚未抬键成员留在实际 ProducerDecision/输入表中，标记 canceled、保存空 child binding；其后原 Up 只提交原 marker 和退休，不触发 Alone/AfterUp/group-after-up。已见原 Up 的成员可立即正常退休。

随后按原设备控制撤销真正失效设备的输入批次。不能删除其他成员的整个输入批次：其同批次其他槽可能仍有未来 Down/Up。取消组内成员不会取消这些独立动作或另一个设备的独立 owner。清理、generation 变化和返回 intents 均为纯逻辑事实，不证明 native accepted ledger 恢复；设备探测/控制鉴权仍在外层。

默认构造和 KW-SIM/runtime gate 保留；自动仲裁/frontier/replay、projection 继承、来源预算、native repeat 与平台验收继续接入。

Complex 设备控制只将已见原 Up 的存活成员作为 released_producers 返回；失效成员沿正常 discarded_input_batches 路径处理。Simple 设备控制先吸收这些实际完成，再撤销失效设备自己的根批次，避免跨设备父绑定滞留；Physical.collect 原有逻辑继续释放完成成员的来源 lease。当前新增回放验证在实际 Complex actor 中完成；完整 Simple/Physical 组路径仍等待自动仲裁和 gate 接通后验收。
