# ADR-0144：Complex 输入 consumed scope 的 FIFO 标记

生成侧已有 consumed modifier 槽隐藏，但向下游只交付 action，导致隐藏差分丢失。增加 Simple 私有 Complex 生成队列模式：非空 scoped plan 捕获原 consumed 槽，open 排在数组前；短作用域 close 排在数组最后，until-input-up 的 close 排在保存 Up 后。所有 action 与 marker 在一个 StageTimeline 事务中按原固定 shift 准入，失败不提交视图、recipe 或交付状态。

DeliveryLedger 保存 scope 的原 slots 和 open/close 交付进度，严格验证 batch/设备/source/旧 Arc/方向/槽集合；close 未交付不得 retire，不能将原动作槽已释放误当作用域完成。Scope 引用另有 16384 预算，单队列批次上界沿用 4096。取消/设备/Reset 清这些记录。旧公开普通队列模式不改变既有调用协议。

本 ADR 完成上游标记及退休协议；Simple 仍对合法 Complex Scope delivery 明确报 UnsupportedSimpleSemantics（错误 stage 报 Invariant），mandatory modifier gate 保留，直到下游模板接收与跨设备取消/恢复完整接线。不是 native 或完整 Simple scope 支持。

单次 action 准入仍限 1024；私有 scope append 仅允许额外最多两个控制，保持原数组最大容量，总时间线仍限 8192。提前 Up 的 close 时间取保存 Up 最后事件时间与输入时间的较大值，避免把标记排到保存 duration 之前。

下一步：Simple 将 scope open 槽列表转为 Complex 后代模板引用，close 精确关闭同组引用，并在 close 交付后重试已完成孩子退休。必须同时处理触发 scope 的设备被移除、被隐藏来源属于另一设备、trigger 已退休但 close 待交付、同一来源交错引用、未知/已失效槽以及部分组准入失败；设备控制不能因取消了 close marker 而永久隐藏其他设备来源。完成这些路径及故障测试后才撤 mandatory gate。
