# ADR-0168：共享组的 descendant projection

任一成员指向 primary 的共享投影模板，每个 ComplexProjectionId 另外保存原引用设备，不能把共享输出的生命周期设备冒充源设备。匹配前 Reserved 成员建立的模板在完整组准入时合并，所有旧 handles 改指向 primary，引用计数相加但身份不改。非空主输出用共享 Down 与 Output born-projected 联合 guards，在首次实际动作之前隐藏键/变量；空 main 保留模板供后续分支。

普通 Alone/AfterUp/Held/Delayed 使用 primary 模板的既有 projected child 准入；全组抬键分支增加同样的 projected dispatch，返回真实 projection ID 加入模板。关闭最后一个引用才恢复仍存活原来源；原 Up 和已完成孩子不因此重新出现。

设备控制已实际取消共享输出后，移除失效引用并调整计数，保留其他设备引用的空模板；不能因为 primary 设备失效就丢掉其他设备引用。实际已取消的 Output projection IDs 从模板中删去，避免后来 close/control restore 使用失效 ID 或重建已取消输出。默认构造及 KW-SIM/runtime gate 保留，完整自动仲裁/frontier/replay、来源预算与 Simple/Physical 联合验收仍须接通。

变量投影继续遵循既有 pause/restore 协议：配置了 key_up_value 时，暂停应用该抬键值，恢复仍存活源时才重新应用保存 Down 值。普通 close 的标记遵循已有输出 FIFO；生命周期 control restore 才绕过未来标记，即时恢复仍存活源并保留真实孩子的未来动作。测试区分这两条路径，不更改既有协议以满足新断言。

最后一个失效引用若属于已经完成的组，仍须清理保存的 Output projection 模板和预算；组完成不等于该模板已关闭。设备控制先清除已实际取消的 projection IDs，然后在必要时显式 control restore 存活模板，只在成功后移除最后引用。保留其他设备的引用及其原身份，后续正常 close 不恢复已经取消的键。
