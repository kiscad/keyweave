# ADR-0180：显式 Linux pointing button 能力

普通mouse设备的 BTN_LEFT..BTN_TASK 尚未走同一引擎，relative Runtime只支持已有keyboard/consumer。选择单独 PointingButtons 冷描述符（最多8个唯一 BTN_MOUSE 连续代码），显式 DeviceSpec.with_pointing_buttons 扩展输出；需同时声明 X/Y 轴，并设置/精确核验 INPUT_PROP_POINTER。默认 DeviceSpec/keyboard normalizer/BackendCapabilities::linux 不变。

映射采用Linux通用mouse/pointer HID路径的 BTN_MOUSE + usage - 1，对应button1..8，标为 EvdevCanonical，不当成已验证 HID descriptor。显式 pointing resolver/normalizer保存Down身份，Up/Repeat沿旧binding，不能用后来的scan重新解释。支持的真实scan必须匹配且RequireHidScan不能忽略。

relative Capture允许这8个code和INPUT_PROP_POINTER，touch/ABS/其他property/unknown BTN继续拒绝；output覆盖和双neutral仍必需。relative Runtime以同一原生normalizer/Physical/pumpledger运行按钮，配置输出native预检按实际DeviceSpec编码；按钮持键影响原Engine pointer modifier保护。键盘和pointer原native顺序不变，停止仅恢复已知按键/按钮Ups。

UI_SET_PROPBIT新增private scalar whitelist并C oracle核验。CaptureGroup/Console与host默认不接mixed scope，graphical consumer权限/交付和硬件验收继续；这是完整pointing路线的增量，不以canonical/能力声明宣称已观察原生接收。

来源：Linux v6.12 drivers/hid/hid-input.c HID_UP_BUTTON/HID_GD_MOUSE 分支 https://github.com/torvalds/linux/blob/v6.12/drivers/hid/hid-input.c；本机linux/input-event-codes.h；https://docs.kernel.org/input/event-codes.html 的button/property合同。无新增unsafe边界/依赖。
