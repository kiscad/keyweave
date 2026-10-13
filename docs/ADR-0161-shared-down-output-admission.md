# ADR-0161：多成员 Down 与实际 Output 联合准入

BindingArena 的组预备持有全部原 Down/payload、主成员的唯一 Output 子树预留和共享保留 ID 预备。原 Down 均须 Reserved、同阶段、来源/旧 Arc/时间正确；共享成员不同设备/快照可保留，但实际主子批次沿第一成员的原 provenance，不能借其身份覆盖其他成员。所有输入和资源校验通过后才给独占 guard。

组 guard commit 一次激活所有参与者，登记唯一主子批次并返回不可 Clone 的共享 handle；abort 返回所有原 owned inputs/payload，不占 ID/引用、不激活、不造输出。它可与 OutputStageActor 的 Generated/Terminal 双预备合成：prepare 两侧都完成，commit 才让组和输出批次可见，任一拒绝/drop 保 arena/输出原状态。

本切片先提供非空主计划的共享联合接口，空 main/投影继承/分支需在实际 Complex 组 actor 中完善。测试通过真实 Output actor 执行已预备键/变量/定时数组并核参与者回收，不做 native 捕获或注入，不声称 OS 接受。KW-SIM/runtime gate 保留；共享组仍需接入仲裁、原 Up any/all、group after-up、device/reset、scope/lease/replay/repeat，不能以本接口代替这些行为。
