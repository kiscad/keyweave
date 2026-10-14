# ADR-0206：公开 X11 启动参数

公开 run-linux 增加 x11 provider，使用显式 --x11-probe、--relative-axes 和可选 --pointing-buttons。轴与按钮为 Linux evdev 十进制代码，逗号分隔；构造现有能力类型，拒绝未知/重复值。X11 必须 staged-keyboard 且不能带 VT；其他 provider 不能携带 X11 参数。helper 必须绝对路径，随后由 PreparedLaunch 验证同 worker 目录的配套文件与权限。

不根据 PATH/环境猜测能力，不自动选设备。省略按钮表示无按钮输出能力。检查在启动或设备操作前完成，沿用现有 cold scope、sealed config、watch/reload 与 session 生命周期。命令行选择不能证明真实 X server、设备 driver、consumer 或权限验收。

测试覆盖实际 clap 解析与 provider 转换及 private argv roundtrip，错误组合均不启动进程、不打开设备。
