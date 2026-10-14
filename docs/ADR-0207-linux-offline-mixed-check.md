# ADR-0207：显式能力的 Linux mixed 离线检查

check-linux 接受与 run-linux 同名 relative-axes/pointing-buttons 参数。共享能力构造与错误规则，轴缺省保持 Keyboard；显式轴选择 StagedRelative，必须 staged-keyboard。按钮要求 X/Y，未知与重复代码拒绝。离线入口不需要 provider/helper/设备路径，不查 session、不创建虚拟设备。

输出诊断打印 capture_scope，仍标记 offline/device readiness unverified。同一 full keyboard output spec 加显式能力，沿用 PreparedConfig 的所有分支输出覆盖检查；仅声明的按钮可用。Linux 纯数据 crate 成为 host 通用依赖，以保留 Windows 上离线 Linux 检查能力；原生模块 cfg 不变。

真实 clap 参数和 PreparedConfig 编译测试覆盖按钮映射、缺少目标输出能力、默认键盘拒绝及模式/能力错误。此验证不能证明系统可捕获或 X11 consumer 交付。
