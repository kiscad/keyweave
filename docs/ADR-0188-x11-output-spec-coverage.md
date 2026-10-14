# ADR-0188：输出 spec 绑定 X11 能力需求

NativeX11Observer.begin 不再接受调用者自报需求，改为 CoverageRequirements.for_output 对实际 DeviceSpec 全量生成键码、semantic buttons、motion/scroll 需求。显式采用 Xorg evdev/libinput 编号模型（键码 offset 8），超过核心 X11 255 的输出直接拒绝，不能截断成可用设备。relative axes 决定 motion 和各滚轮方向需求，高分辨率轴要求相应 scroll metadata，但不证明数值缩放或实际投递。

schema 4 增加 button_labels：按 X11 按钮顺序列出对应 Linux BTN_LEFT..TASK native code，未知标签为 0。只查八个固定 semantic label atoms，读取 class label 数组而不读 pressed mask；source/count/重复 class/已知标签歧义拒绝。消费者严格验证长度、白名单和已知标签唯一性。输出按钮检查依赖标签存在，不将 HID 顺序误作 X11 顺序；旧 schema 1..3 保留既有范围，但不能提供按钮语义证据。

编号模型依据 [xf86-input-libinput 的 offset 和按钮转换](https://raw.githubusercontent.com/whot/xf86-input-libinput/master/src/xf86libinput.c)，标签依据 [X server properties](https://raw.githubusercontent.com/mirror/xserver/master/include/xserver-properties.h)。此选择是明确的 Xorg 模型，不是对任意未知 driver 的证明；driver识别、可变映射/设置、SDK/live-server、投递和 capture admission 仍需后续核验。没有连接显示/捕获设备。
