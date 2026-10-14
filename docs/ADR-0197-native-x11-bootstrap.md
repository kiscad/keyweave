# ADR-0197：原生 X11 mixed bootstrap

新增 X11Bootstrap，冷 Plan 固定 StagedKeyboard+relative spec，Kernel repeat/输入身份/config coverage 先于任何 OS打开。实际 platform 使用 guarded NativeX11Observer、owned Uinput/Evdev output node、显式relative Evdev capture source和唯一helper；共用Output/Discovery/Inputs/Group/Arm/Verify流程，mixed group含emergency keyboard检查。pending与unknown分开，post-arm不得越旧证据expiry。

成功 finish 将同一platform此前取出的exact observation放回同一observer（crate-private，仅bootstrap handoff），随后 NativeX11Lease消费，携 Runtime和同一MonotonicClock给X11Prepared。无Clone/重新mint或新观察起点。尚待helper时不得handoff。

错误/取消先关闭输入与输出，再cancel并保platform，finish_preserving返回原bootstrap owner；X11StartupCleanup持续poll同一child，确认无pending job才drop observer/node并报告。重复close不重做事务。Drop仍best-effort，正式startup调用者必须into_cleanup并确认，不把丢弃当完成。

该API需调用者已建立独立监督并在每step前处理Stop；host/session默认选择尚未接入。helper认证、driver policy、真实session/server/hardware验收仍独立待完成。原生入口仅编译，测试使用共用mixed状态机/retained平台mock，不打开设备或连接显示。
