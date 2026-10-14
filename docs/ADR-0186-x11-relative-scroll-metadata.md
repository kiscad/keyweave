# ADR-0186：X11 相对滚轮能力元数据

schema 3 增加 relative_scroll_axes：0 horizontal，1 vertical。隔离 helper 读取 XIScrollClass 的编号、方向、increment/flags，将其关联到同一 slave 的相对 valuator 编号。increment 必须有限非零，方向/flags/编号须合法，不读取当前位置或修改任何 server 状态。单方向重复、重复轴编号、缺相对 valuator 拒绝，不把按钮 4..7 当滚轮类证据。

消费者 schema 1/2 继续支持既有检查，但只能 schema 3 提供滚轮证据；schema 3 每设备显式提供 motion/scroll 字段，轴数组有界有序 pointer-only。with_relative_scroll 需求可选择 horizontal/vertical，并可同时要求 motion/按钮，全部须同一个 slave 覆盖。只证明 server metadata，不证明高分辨率缩放、实际事件投递或 capture 授权。

类标记、方向和 flags 数值依据 [XI2 协议头](https://raw.githubusercontent.com/freedesktop-unofficial-mirror/xorg__proto__inputproto/master/XI2.h)。布局参考 [libXi 公共 SDK](https://raw.githubusercontent.com/mirror/libXi/master/include/X11/extensions/XInput2.h)。扩展真实 SDK oracle 对 scroll class/layout/constants 的断言；本环境缺 SDK headers，原生 ABI 和服务器仍未验证。没有放宽 loader root trust。消费者会话/输出节点/TTL 整合和真实硬件验收仍为后续工作。
