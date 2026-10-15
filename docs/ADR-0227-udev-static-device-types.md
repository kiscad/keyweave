# ADR-0227：Linux udev 静态设备类型来源

问题：ADR-0226的四字段设备selector需要明确keyboard/pointing正反事实，当前Evdev只提供vendor/product。不能由名称、eventN编号或部分键码猜类型，也不能在没有分类记录时把属性缺失当false。

依据官方[libinput静态udev配置](https://wayland.freedesktop.org/libinput/doc/latest/device-configuration-via-udev.html)：类型标签是设备添加时读取的静态分类；ID_INPUT与至少一种类型标签用于设备类型确定，但标签不证明设备可被libinput初始化。参考[systemd input_id源码](https://github.com/systemd/systemd/blob/main/src/udev/udev-builtin-input_id.c)：ID_INPUT为分类标记。读取libudev公开API；不执行udevadm/builtin/rules/user commands。

选择：EvdevDevice::open冷阶段在已有FD、内核ID和canonical input_syspath确定后，按该FD的dev_t建立libudev设备对象，核对devnum、subsystem=input、sysname=eventN、其input父syspath与当前Probe一致。只接受initialized、ID_INPUT=1、至少一种已知类型标签=1、所有读取标签为缺省/0/1的完整静态标签快照。此完整分类范围内，未标记的类别表示false；没有初始化/ID_INPUT/正类型记录，属性缺失或无效时整个type observation为Unknown，绝不补false。

Linux契约：is_keyboard来自ID_INPUT_KEYBOARD；is_pointing_device为MOUSE/TOUCHPAD/TOUCHSCREEN/TABLET/TABLET_PAD/POINTINGSTICK的OR。这是udev管理员可配置分类，**不是macOS HID角色相等或真实libinput可用性证明**。KEY/JOYSTICK/ACCELEROMETER/SWITCH仅用于确认有分类结果，不伪造consumer/built-in等属性。类型是本次已打开设备实例的冷快照，不监听规则变化；更改udev规则后需重新打开会话。拔除/代际仍受实际CaptureSource绑定及Runtime现有规则管理。

动态libudev只从架构固定系统路径加载，canonical文件与全部ancestor须root owner且无group/world写，沿既有sd-login边界，不因沙箱uid映射放宽。窄FFI持有library→context→device所有权，所有opaque调用和字符串指针限定在其生命周期；不导出raw句柄。ABI依据官方固定 [systemd v257 libudev.h](https://raw.githubusercontent.com/systemd/systemd/v257/src/libudev/libudev.h)的公开标量/不透明指针，固定符号名；非Linux不引入FFI。同时要求/run/udev/data/cMAJOR:MINOR分类记录及所有ancestor为root owner、不可group/world写、非symlink、regular文件不超过256KiB；这固定了目前systemd数据库布局，其他布局保守Unknown。对象/路径/ID核验前后任一失败或provider不可用均保留Unknown，不阻止普通全局映射。

CaptureSource增加缺省None的类型快照接口；真实EvdevSource交付冷快照，Runtime复制到已有DeviceContext。只在open时做系统查询；运行时poll/callback不加载库、不读udev、不执行文件IO，不自动刷新静态分类。测试需覆盖分类前提/无效值/组合类型、native对象核验与释放路径、实际Runtime→两模式设备映射；原生库/硬件验收独立记录。无抓取/注入授权扩展。

本轮实际验证：1927工作区测试及既有格式/Clippy/源码61/C UAPI/X11 SDK检查通过。只读loader示例exit1，系统库及祖先在沙箱下显示UID65534，因此native device查询未执行；没有放宽root trust。libudev.h本机缺失，SDK签名按官方固定v257header审阅，既有C ABI测试不覆盖新增opaque API。原生对象/数据库代际行为、硬件分类和不同发行版布局仍需真实验收；root信任与SDK initialized记录不构成完整管理员竞态/映像认证证明。
