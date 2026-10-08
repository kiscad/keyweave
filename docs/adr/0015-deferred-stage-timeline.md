# ADR-0015：延后映射工作的阶段时间队列

状态：独立队列已实现并离线验证；引擎 actor 接入未完成。2026-10-08。

## 问题

ADR-0014 的末端输出 FIFO 不能执行下一阶段规则。把所有阶段放在一个 FIFO 又会使 simple 生成的未来 complex up 阻塞 complex down 派生出的当前 output down。需要每阶段独立保持因果顺序，并把已规划时间与实际交付时间分开。

## 选择与实现

engine 的 `timeline` 模块提供 StageTimeline<T>，Simple/Complex/Output 各一 lane。同 lane 后续工作最早时间不早于其前一项；跨 lane 选择最早可执行项，同刻按入队顺序。每条 StageEvent 保留 ConfigSnapshot Arc、原 DeviceInstanceId、SourceToken 和 payload，执行时不替换为当前配置。DueEvent 保留计划 at_ns 与 delivered_at，条件/provider 有效性必须按实际交付时间求值。

总量 8192，单 append 1024，设备历史沿用 4096 上限；批次预检容量、设备代际和排序序号溢出，失败返回完整原 Vec，不消耗其中 payload。排序序号递增，不随 Reset 归零。同一 actor 执行，不引入线程、锁、OS API 或系统时钟。泛型 payload 是后续命令容器，队列只证明项数有界；actor 还需限制 payload 大小与 owner/token/suppression 的生命周期预算，不能放任意用户数据。

设备控制应先于同刻普通队列工作；observe_device 的更高代际取消该身份的旧工作，移除取消同代工作，旧代请求拒绝，已移除同代不能复活。不同身份保持独立。discard_source 精确匹配 device+source，禁止键码/时间关联猜测。Reset 返回全部未执行工作，保留设备历史及排序序号。返回取消项按有效时间与入队顺序排列。

## 接入要求与限制

目前 Engine::press_stage/press_actions 仍递归执行；没有用此组件声明 hold_down_milliseconds 可执行。后续把 action down、安排的 up、modifier scope、变量 up 和后续数组动作表达为 actor 命令，关联原 binding/旧配置与已预留 source range。末动作 repeat=true 保持至输入 up，hold duration 只影响安排的轻触；新动作批次的条件仍在生成阶段整批筛选，下一阶段规则在该阶段实际交付时求值。

需为因果父子工作和外部同刻 input/timer/context 明确 actor 调度顺序；该组件的稳定排序仅覆盖已入队工作，不代替完整 pipeline 的优先级。新增事件不能把一条 stage 队列的未来边沿变成当前 owner 改动。队列 pop/discard 不直接触发输出，actor 必须区分尚未执行的任务预算与已执行 down 的 owner 清理。设备移除、Reset、热更新、alone 快照和 halt 延续需共同测试。输出队列仅接收执行完最后阶段的 OutputIntent。

9 个测试覆盖阶段间推进与 lane FIFO、同刻稳定排序、旧配置/source/设备保存、迟到时间、作用域取消、代际替换/移除、完整批次拒绝、队列/历史容量和 Reset/单调时间。Windows/系统输出/硬件未验证；上游原 fixture 不修改。
