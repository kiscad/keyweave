# ADR-0005：有界设备代际历史

状态：采用；2026-10-08。

问题：将所有断开过的 `(id,generation)` 存在退休集合会随反复插拔无限增长。旧 DeviceRemoved 不能误删新连接；新代际即使没有按键事件就移除，也应释放其前一个连接留下的 owner。滚轮/移动事件同样不能绕过退休保护。

选择：每个设备 id 只保存最高 generation 与 removed 标志。新连接/新连接移除都会释放旧连接的阶段 binding，但不触发宏。旧移除通知幂等忽略；旧 key/wheel/move 输入明确拒绝。已移除的同代际不能复活；必须提升 generation。Reset 保留代际保护，只有输出所有权与宏预算被清空。

资源：最多 4096 个不同设备 id，每个 id 常数大小的历史；一万次相同 id 重连仍只留一条。id 是运行时注册表的设备身份，generation 单调提升，不能仅复用 OS 裸句柄而不更新代际。耗尽不同 id 限额返回 `DeviceHistoryFull`，不会隐式丢弃历史后接受迟到的输入。

恢复契约：运行时应先 Reset/释放输出、停止抓取、排空旧输入，等 neutral 后重建引擎与设备注册表。单独 Reset 不删除历史，不能被描述为已恢复设备 id 容量。目前没有 OS runtime，这一恢复路径尚需后端阶段实际实现和故障注入。

证据：engine/src/lib.rs 的 update_device；lifecycle.rs 的 stale remove、新代际无 down 移除、wheel/move 退休检查、10000 次重连与 4096 身份限额测试。与物理 binding/输出 budget 分开限制，未测试真实热插拔。
