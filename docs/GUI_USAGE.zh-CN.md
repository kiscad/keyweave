# KeyWeave 原生配置编辑器

运行 `cargo run --locked --offline -p keyweave-ui` 打开 Rust 设置窗口。界面采用固定版本 egui/eframe，包含 X11/Wayland 和 Windows 窗口支持；当前文字界面为英语，中文字体、可访问性与托盘仍待完善。

1. 在 **Open configuration** 输入配置路径，点击 **Open**。支持 UTF-8 JSONC，保留注释与原始换行，读取上限 8 MiB。替换未保存编辑时会要求明确确认。
2. 选择 Linux/Windows 和 keyboard/keyboard + mouse 目标；**Timed rules and chords** 对应 staged 模式。混合目标要求启用该模式。Linux mouse 离线检查使用完整支持的输出表，实际设备准入仍由运行会话负责。
3. 编辑配置，点击 **Validate**。校验复用现有编译器与 worker 冷准备，显示错误代码、严重性、JSON Pointer 和原因。校验在单独线程执行；修改内容、模式或目标会使旧结果失效。
4. 当前文档验证通过后，在 **New copy path** 输入新路径，点击 **Save validated copy**。副本保存完整 UTF-8 字节，不覆盖已有文件或符号链接。保存先完成同目录临时文件写入与 sync，再以 hard link 发布；文件系统不支持时返回错误。临时文件清理失败会独立显示。

窗口关闭时会提示未保存编辑。会话启动和停止继续使用 [Linux CLI](LINUX_USAGE.zh-CN.md) 或 [Windows CLI](WINDOWS_USAGE.zh-CN.md)；当前编辑器没有运行时连接，也不会把离线验证显示为“已应用”。配置导入向导、设备列表及受监督的会话控制仍待接入。

`keyweave-ui --smoke` 用于创建窗口并在三个 UI pass 后请求关闭，不安装键盘/鼠标 Hook，也不注入输入。当前 WSL 环境的窗口测试分别返回 XOpenDisplayFailed/NoCompositor；headless 布局、文件行为与校验模型已测试，不能据此宣称实际桌面显示或 Windows GUI 已验收。参见 [ADR-0222](ADR-0222-native-settings-ui.md) 与[验证记录](validation/linux-2026-10-15-native-ui.json)。

点击 **Edit profiles and simple mappings** 打开可视化草稿。下拉菜单只浏览 profile；**Select as active** 修改配置中的 selected，不激活运行会话。可添加 profile，编辑名称后点击 **Rename**；简单映射填写键名（如 a、caps_lock、consumer:mute、button1），点击 **Update** 或 **Add mapping** 提交到草稿。**Remove** 删除对应简单行。包含 modifiers、多目标或其他高级字段的行显示只读提示，请在 JSON 中修改。

点击 **Use edited JSON** 才把草稿写入文档：会移除注释、重排格式，但保留未编辑的 JSON 值、未知字段与复杂规则；随后必须重新 Validate。若原始文本已变化，旧草稿会拒绝应用，请关闭草稿后重新打开。**Close visual editor** 对已修改草稿要求确认丢弃；打开文件和关闭窗口也检查草稿修改。未点击 Rename/Update/Add 的输入框内容只是待提交输入。**Save validated copy** 保存当前文档，未应用的草稿不会进入副本。

可视化界面上限为 128 个 profile、每 profile 4096 条简单映射，超过时保留文本编辑功能，拒绝可视化加载而不截断数据。参见 [ADR-0223](ADR-0223-visual-profiles-and-simple-mappings.md) 与[验证记录](validation/linux-2026-10-15-visual-ui.json)。
