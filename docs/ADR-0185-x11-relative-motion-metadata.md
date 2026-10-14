# ADR-0185：X11 相对移动元数据

schema 2 增加 relative_motion_axes 数组：0 表示标签 Rel X，1 表示 Rel Y，只记录 XIValuatorClass 的 XIModeRelative 类。不读取 value、坐标、按键或事件。查 atom 使用 only-if-exists，不创建属性。未知标签、绝对轴不产生相对证据；无效 number/mode、重复语义轴拒绝。所有解析仍在隔离进程。

消费者保留 schema 1 的键盘/按钮兼容，但 schema 1 不得携带非空新增轴字段。schema 2 的每个设备必须显式携带该字段，避免缺字段假装完整探针。数组限定两项、递增、pointer-only。CoverageRequirements 显式 with_relative_motion 要求同一 pointer 同时具有 X/Y 及所需按钮；空 motion-only 需求合法，无隐式滚轮支持。完整授权仍需 owned 节点、会话、inhibited 和 TTL。

公开布局依据 [libXi SDK](https://raw.githubusercontent.com/mirror/libXi/master/include/X11/extensions/XInput2.h)；标签依据 [X server 属性定义](https://raw.githubusercontent.com/mirror/xserver/master/include/xserver-properties.h)，libinput 的 [相对轴初始化](https://raw.githubusercontent.com/whot/xf86-input-libinput/master/src/xf86libinput.c) 使用相同标签。扩充真实 SDK C oracle 的 valuator 布局，缺 headers 仍应失败，不以 Rust fixture 代替 SDK 验证。滚轮 scroll class、实际服务器和映射编译继续待完成。
