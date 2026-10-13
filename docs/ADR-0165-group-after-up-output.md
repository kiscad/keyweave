# ADR-0165：全组抬键分支的保存计划和拥有者屏障

上游 basic.hpp 510–646 行区分普通 to_after_key_up 与 simultaneous_options.to_after_key_up。前者在 any/all 主输出首次释放时执行，后者在全组原 Up 时执行。全组分支用当前修饰键减 mandatory consumed，不恢复 Down 修饰快照；条件使用当前状态和最后抬键设备。

RuleBranch::GroupAfterKeyUp 从保存的完整 simultaneous 定义取动作；BranchSelection 按当前条件过滤一次，保留旧配置 Arc，作为 tap 数组复用 Output 的保存 scope 和时长执行。普通 actor 增加分支元数据识别，不自动触发该分支，也不扩大生产 gate。

GroupBindingOwner 在全组 Up 事实出现时记录一次 pending 分支和实际最后抬键成员。finish 即便调用方声称 outer_work_done 也不能跳过该 pending。dispatch_group_after_up 以旧主成员 selection 和最后 Up 成员的条件身份，在显式实际生成时间一次过滤和联合准入子计划；全过滤/空分支终结一次，无假输出/孩子。完整子准入 commit 后才清 pending；失败 latch Failed 并保留共享 hold，交给显式控制清理，不能重试重新选择。

Output 子计划生命周期继续归主成员保存 parent/device/source，以满足已有 child provenance；条件身份取最后 Up 成员。这两种身份不得混淆。未来统一组 device control 须任一成员失效都处理整个组和后续输出。返回真实 child ID 与生成 halt 事实供外层调度；不自动退休 child，不声称 OS 接受。成员保持至所有实际孩子和外层工作完成。

生产 Complex 统一组调度、普通分支与 group 分支先后、投影、控制、回放、来源预算和 repeat 仍须接入，KW-SIM/runtime gate 保持。初始 lookahead 已有全部 Up 时最后成员从原 owned 事件顺序提取，实际 ProducerDecision Up 仍须先准入。
