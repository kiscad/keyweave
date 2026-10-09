# ADR-0032：Windows Hook 的 scan 身份与注入编码

## 选择

新增 keyweave-windows crate，纯安全 Rust，先实现 native records/键表/normalizer/KEYBDINPUT 语义 packet，不安装 Hook、不调用 SendInput。使用 Microsoft 的 Scan 1 Make→HID 数值事实；输入为 scan byte 与 LLKHF_EXTENDED，字母 VK 不决定物理位置，E0 不从低字节猜测。

事件先校验 WM_KEY/SYSKEY down/up 与 LLKHF_UP 一致、VK/scan 范围及已定义 flags。injected flag 与非零本进程 marker 同时匹配才视为 OwnInjected；其他 injected 默认 bypass，marker 单独出现不是来源证明。之后按固定 512 个 native slots（byte×E0）保存 down 身份，重复 down 为 Repeat，up 使用旧 binding；注入 up 不影响物理 held，reset 不编造 up。Hook 缺可信设备 ID，此状态是会话合并流，不承诺每键盘 owner。

按标准 scan 选 canonical HID，backslash/non-US hash、F24/LANG5 等碰撞附 known_aliases；StrictUnique 模式拒绝歧义。它是 CanonicalScan evidence，不证明原始 HID。非 canonical HID 输出明确拒绝。PrintScreen/SysRq、Pause/Break、NumLock 与 LANG1/2 的特殊/仅 release 序列保留 dedicated gate，后续必须按真实 Hook 与注入结果实现，不能伪造单字节支持。Consumer/Unicode/Fn/电源不是普通 keyboard scan MVP，驱动/专门动作继续待做。

输出 packet 使用 VK=0、KEYEVENTF_SCANCODE、E0 flag 与 key_up，time=0 和 marker；Repeat 编码为再次 down。有界整批验证，不注入、不将编码完成视为 SendInput 成功。后续 native FFI/消息线程/同步 capture 决策、submitted ledger、UIPI/会话恢复单独接入。

## 验证与证据

portable tests 验证普通键表唯一 canonical（特殊项仅记录 primary byte，不作为完整序列）、左右 modifier/numpad/navigation、VK 随布局变化时 saved up、repeat、injected bypass、flags/message 错误原子性、特殊/歧义 gate 与 packet flags。Linux 上这些测试不能证明 Windows 编译、Hook 拦截或 SendInput 真机成功；Windows CI 配置继续保留、实际执行单独记录。

数值依据：[Microsoft Scan Codes](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-keyboard-input#scan-codes)、[KBDLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct)、[KEYBDINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput)。键表仅整理数值事实，没有复制第三方实现代码。
