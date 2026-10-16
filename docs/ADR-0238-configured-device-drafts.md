# ADR-0238：配置设备选择器与专属 simple 草稿

在既有 VisualDraft 上增加 Configured devices 编辑，不将配置项称为已连接硬件。支持选择/新增、vendor/product u32（十进制或0x）、keyboard/pointing 精确布尔标志、simple行编辑/新增/删除，以及整个device的二次确认删除。类型缺省0/false沿用既有compiler/upstream语义，不从名称或按键推断。

每个DeviceEdit保留完整expected Value与原始device；修改仅落本地buffer。Commit完整验证所有ID/命名按键/限额后一次替换，失败不改变VisualDraft；未修改字段、未知metadata/硬件字段、advanced rows和其他profiles保留。仅发生selector编辑时写四个已知字段，未修改行保持原始JSON（包括别名/省略值），不静默标准化。期望device值与新建位置fence拒绝过期覆盖/删除。

设备最多256；profile全局+设备simple合计4096，完整complex/其他预算仍由离线compiler验证。形状错误/超限需要原始JSON修复，不截断。待提交device buffer参与Settings unsaved、Use edited JSON/关闭及profile切换门槛；整个文档仍由原始source fence保护。Commit是设备buffer→visual draft，Use edited JSON是draft→document，不是运行时Apply或文件覆盖。

当前Windows Hook缺可靠逐设备身份，Linux类型provider也需实际可信metadata；GUI不扩大backend能力，不声称硬件可用。connected-device discovery/ignore/LED/Fn硬件设置、运行时设备状态、原生窗口/硬件与完整目标继续。

验证：新增6项测试，覆盖unknown/hardware/advanced/其他profile保留与no-op、ID/row错误与过期编辑/删除/append原子性、4096共享budget/256设备与形状错误不截断、Settings pending/unsaved/验证撤销/source fence、实际离线compiler与完整冷准备的target/mode gates、设备面板headless布局无修改。Linux全量2004通过，8已有native IPC默认ignored；格式/Clippy/源码62/Linux ABI/X11 SDK/regex oracle通过。未运行真实窗口、物理设备或Windows原生 gate。见 [验证记录](validation/linux-2026-10-16-configured-device-ui.json)。
