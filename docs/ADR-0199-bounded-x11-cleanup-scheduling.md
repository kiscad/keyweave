# ADR-0199：启动/运行后 cleanup 的有限调度

提取 crate-private Task/Owner/Schedule，共用500ms窗口、2ms唤醒和512次上限。Task初始化接受真实时钟Result，失败时last/deadline为None并保clock error，无伪造时间。NativeSchedule只用独立monotonic clock与空FD reactor等待，不读输入或重新查询consumer。

X11Cleanup.run与X11StartupCleanup.schedule→ScheduledX11StartupCleanup.run均保同一owner：完成后finish确认/drop，截止/clock/poll/cancel/wait/attempt故障则返回boxed task，无新probe或bootstrap。wait错误后可做一次同owner退出观察，记录不被实际exit抹除。clock/wait errors独立于Runtime及原cancel结果；初始clock失败仍可确认已退出child。

启动清理成功报告仍不是Runtime termination；startup selector/session报告后续连接。Native循环仅编译，测试注入调度clock和已取消owner，验证有限次数、期限、wait错误和确认后的资源处理。原有实际helper回收测试继续。没有真实session/display/input/uinput操作。
