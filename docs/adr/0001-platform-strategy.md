# ADR-0001：平台实现与验证边界

状态：采用。日期：2026-10-08。

先实现独立 Rust 类型、配置编译、确定性回放和规则引擎，再接 Linux evdev/uinput 与 Windows 会话内 Hook/SendInput。GUI、会话控制与独立 Windows Rust 驱动继续按 D09–D11 开发，不从最终目标删去。

Windows Hook 不提供可靠逐事件设备身份，Raw Input 不与 Hook 按时间戳配对；不支持的条件禁用整个规则。VHF 只提供输出，物理屏蔽另需过滤驱动。

当前执行机为 Linux，Rust 1.97.1；无 `/dev/input` 或 `/dev/uinput`，仅安装 Linux Rust target。离线测试与本机编译可以执行，真实设备和 Windows driver 加载不能由此证明。每阶段分别记录实现、编译、回放、真机结果；缺目标平台不阻止开发独立部分。

选择 MIT 作为新 Rust 源码许可证；保留来自 Karabiner 的 Unlicense fixture 和 pqrs HID 数据的 Boost-1.0 来源声明与许可证，不能将它们重新标为 MIT。
