# ADR-0019：Linux 键身份与 HID 信息损失

状态：转换/规范化与引擎联合验证已实现；设备/配置准入和真实 backend 未接入。2026-10-08。

## 依据与表来源

本项目标准 HID 名字/usage 与本机 `/usr/include/linux/input-event-codes.h` 的 KEY_* ABI 常量人工配对，保存独立数值事实 data/linux-key-facts.json。tools/generate_linux_keys.py 校验并生成只读 Rust 表，--check 无需 Linux header，--header 可核验本机常量。没有复制 Linux C 实现或整个 hid_keyboard 数组，不更改原 Karabiner fixture。

核验 [Linux v6.12 hid-input.c](https://github.com/torvalds/linux/blob/v6.12/drivers/hid/hid-input.c) 的标准 HID 转换与 [UAPI key codes](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/input-event-codes.h)。EV_KEY code 并非原始 HID usage，driver quirks、keymap 修改及多个 usage 可使信息损失。kernel 的标准表中 non-US pound/backslash，以及 keyboard/consumer 音量键存在共享 native code；不能把 evdev 身份标成原始 HID 身份证明。[hid-input.c](https://github.com/torvalds/linux/blob/v6.12/drivers/hid/hid-input.c)

## 行为

159 HID/native 对，154 canonical native codes。标准字母/数字、位置键、左右修饰、numpad、F1–F24、部分编辑/国际键与 12 个 consumer 用途有 canonical 输出。五个表内别名（non_us_pound、keyboard mute/volume_up/volume_down、clear）只允许可信 HID scan 输入解析，output_code 返回 OutputAlias；不静默伪称原 HID usage 可以由 uinput evdev 恢复。未支持页/usage/code 明确错误，鼠标和系统/vendor/Fn 扩展仍未实现。

resolve_input 接收可选可信 HID scan。scan 必须由已核验 driver/descriptor 路径提供，不能按整数形状把任意 MSC/PS2 scan 当 USB HID。scan 与 native code 不一致拒绝。没有 scan 时只有明确 CanonicalEvdev policy 能选择规范身份，返回 EvdevCanonical evidence；RequireHidScan 对所有 code 都要求 scan，包括表面唯一 code。has_known_aliases 仅描述当前表中已列出的别名，不证明硬件 usage 唯一。默认 runtime policy 仍待准入设计，此函数没有自行启用抓取。

KeyNormalizer 每个 device generation 一个实例，以 native code 保存按下时 ResolvedKey。Repeat/duplicate down/up 复用同一个 saved identity，扫描缺失/变化不重新确定 up；同键不同来源的最终 ownership 仍由引擎负责。未知 up/repeat 返回 Ignored，不猜原 down 或释放别人。无效 event/新 down scan 错误不改变现有绑定；reset 只清本地状态，生命周期调用者另以设备/代际释放原 engine bindings，不制造假 up。

## 接入要求

Linux BackendCapabilities 的粗粒度 HID 页声明仍不是 native 全表可用证明。设备 probe 与配置激活必须结合具体表、canonical policy、scan 来源及所有可达输出（包括原样透传），对 unsupported/alias output 提供诊断，不能在抓取后才遇到无法输出的键。可信 scan 的 alias 输入若没有可执行映射/透传准入方案，也不能默认抓取。具体 UAPI 底层、own virtual 排除、neutral/grab、LED/repeat/watchdog 与跨 session 仍待实现。

消费者 canonical 输出与 keyboard 同名 HID 不能混为一种逻辑键；例如 consumer mute 能输出 native 113，keyboard mute 在此层明确不可保真输出。Windows Hook 无可靠设备身份的限制不因这张 Linux 表改变。国际键是标准 HID usage 对应关系，不承诺 macOS 别名对应的输入法/布局效果。

## 验证

10 个转换/状态测试覆盖全部 159 对、154 canonical 唯一性、左右/普通 Enter/numpad 区分、音量/反斜杠别名、严格 scan、保存 down 身份、孤立 up/repeat、坏 event 原子性、设备实例分隔及全 code 容量。另 1 个 native frame→normalizer→Engine→native output 联合测试验证 a→b、repeat 无新宏、热更新后原 up 仍释放 b。generate --check 和本机 header 核验通过；无系统 fd/硬件/Windows 运行。
