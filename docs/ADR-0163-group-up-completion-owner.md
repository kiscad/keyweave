# ADR-0163：共享组原 Up 与输出完成拥有者

新增组拥有者组合 ActiveChord、共享 handle、成员实际 ProducerDecision 与主输出批次：原 Up 先通过保存 binding 凭据准备，再更新组事实、按 any/all 提交保存 Output input_up，最后提交原 Up marker。任何后续失败 latch Failed，不能用组事实清实际输出/输入绑定；只读来源错误在修改前拒绝。

拥有者消费实际 Output ActorStep 的完成报告，仅在该批次 recipe 确已消失后清主输出等待。普通外部输出完成不能冒充此组完成。共享 handle 不在 Drop 或原 Up 时释放；显式 finish 还必须得到全成员原 Up、主输出已完成、参与者没有其他孩子/保留及调用方确认全部外层分支/作用域调度结束，才解除 handle 并取走实际成员 payload。返回 payload/槽给外层输入表回收，不自动退休整个输入批次，不声称 OS 接受。

本组件是 Complex 组 actor 的拥有者状态，不另建默认捕获入口；匹配、原输入登记、空 main、多分支/projection/设备/reset/replay/repeat 的统一调度继续接入。初始 lookahead 内已有 Up 必须先在实际 producer 上按当前准入 clock 提交，拥有者核对它们后处理 initial facts，不伪造原 earliest。

另补 Phase0162 后的边界：Complex.activate 必须继续严格校验输入执行功能，不能因为其内部 Output 支持完整 metadata 而接受无法执行的组配置。KW-SIM/runtime gate 保留。

构造同时核对主成员保存 selection 的 Complex 阶段、原 snapshot Arc 与完整 simultaneous 定义，不能将另一个规则匹配附着到现有输出上。重复完成报告不重复推进。失败拥有者可通过 into_control_ownership 将共享 handle、成员槽和主批次移交外层控制；移交本身不释放任何实际工作，控制器仍须分别撤销输出、解除保留和处理绑定/输入表。该接口不是完整 Reset/device 调度实现。
