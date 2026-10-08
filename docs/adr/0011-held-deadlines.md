# ADR-0011：显式可撤销按住 deadline

状态：已实现离线 held 切片；2026-10-08。

映射 down 保存原配置、源设备/代际与 deadline token；阈值到达时先完成旧 to 子 binding 的 up，再以当前条件执行原配置的 to_if_held_down，原物理 up 释放新 children。alone/after 仍按原时序，held 不自动取消 alone。其他新物理 key/button down 取消所有尚未触发的 held 任务，repeat/重复 down/move/wheel 不取消。simple 生成的每一个 complex down 也会取消同阶段较早的任务（包括同批 modifier scope），对应上游 cancel 在 validity 检查之前执行。

计时由外部单调时间与 TimerFired{token} 驱动，没有 sleep、线程或 OS clock。Engine.next_deadline 返回时间/token，EngineStep.next_deadline 保持时间接口；运行时必须在下一个普通物理事件之前提交所有到期 token，等阈值时 timer 在 key up 前处理。尚有到期任务时普通物理事件返回 DeadlinePending；Reset/remove 优先取消，context 更新可先于同刻 timer，使动作看到新的属性。提前 token/乱序 token 拒绝；过期 token 幂等无输出。晚交付 timer 按实际交付时间执行和判断条件，不回写过去输出时间。

每个任务预留一个 SourceToken 区间单元，转为窄 DeadlineToken；队列按时间/token 有序，容量受全局 reservation 16384 限制。原 down 预留所有 held 输出/派生 up/宏/suppression 的最大展开，timer 不重新申请 owner 预算。设备移除、子 binding 结束、Reset 删除任务，配置更新保留 active 原任务。单个 TimerFired 只触发一个任务，最多原 binding 的输出限额，运行时循环提交下一个 token，避免同刻多个宏突破事件上限。

阈值时间相加溢出必须在新 down 输出前拒绝，采用当前配置最大 held 阈值的保守检查，回调还检查其保存配置的阈值，时间空间不足时需 Reset 恢复；zero threshold 仍显式投递 timer，不在 down 回调递归触发。没有 delayed_action/simultaneous/hold duration，此 ADR 不声称完整时间规则已实现。

验证：12 个 engine、2 个 compiler、1 个 CLI 新测试，workspace 132 个通过；阈值前/等值/零、repeat/取消/轮移、原 context device、热更新、拔除/Reset、重连/旧 token、时间溢出/展开预算、simple→complex 替换和同批派生取消、mandatory scope、alone/after 顺序均覆盖。上游 to_if_held_down.hpp 原在 source manifest，现全表仍 60 hash；macOS 原测试未执行，原 oracle 未改。

运行时必须自行分配统一 engine seq：先交付 due token，再给等待的 native 事件分配其 envelope seq；不能先给 native envelope 分配旧 seq，再插入更大 seq 的 timer。相同时间允许但 seq 严格增加。实际 backend timer、回调线程、停滞 watchdog 和 late delivery 性能未实现/未验证。
