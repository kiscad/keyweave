# KeyWeave 原生配置编辑器

运行 `cargo run --locked --offline -p keyweave-ui` 打开 Rust 设置窗口。界面采用固定版本 egui/eframe，包含 X11/Wayland 和 Windows 窗口支持；当前文字界面为英语，中文字体、可访问性与托盘仍待完善。

1. 在 **Open configuration** 输入配置路径，点击 **Open**。支持 UTF-8 JSONC，保留注释与原始换行，读取上限 8 MiB。替换未保存编辑时会要求明确确认。
2. 选择 Linux/Windows 和 keyboard/keyboard + mouse 目标；**Timed rules and chords** 对应 staged 模式。混合目标要求启用该模式。Linux mouse 离线检查使用完整支持的输出表，实际设备准入仍由运行会话负责。
3. 编辑配置，点击 **Validate**。校验复用现有编译器与 worker 冷准备，显示错误代码、严重性、JSON Pointer 和原因。校验在单独线程执行；修改内容、模式或目标会使旧结果失效。
4. 当前文档验证通过后，在 **New copy path** 输入新路径，点击 **Save validated copy**。副本保存完整 UTF-8 字节，不覆盖已有文件或符号链接。保存先完成同目录临时文件写入与 sync，再以 hard link 发布；文件系统不支持时返回错误。临时文件清理失败会独立显示。

窗口关闭时会提示未保存编辑。会话启动继续使用 [Linux CLI](LINUX_USAGE.zh-CN.md) 或 [Windows CLI](WINDOWS_USAGE.zh-CN.md)。Linux/Windows host可显式启用本地Status/Stop，GUI连接方式见下；Windows专属工厂与界面仍待原生验收。不会把离线验证显示为“已应用”。已连接设备列表、会话Start/Resume/Apply仍待接入。

`keyweave-ui --smoke` 用于创建窗口并在三个 UI pass 后请求关闭，不安装键盘/鼠标 Hook，也不注入输入。当前 WSL 环境的窗口测试分别返回 XOpenDisplayFailed/NoCompositor；headless 布局、文件行为与校验模型已测试，不能据此宣称实际桌面显示或 Windows GUI 已验收。参见 [ADR-0222](ADR-0222-native-settings-ui.md) 与[验证记录](validation/linux-2026-10-15-native-ui.json)。

点击 **Edit profiles and simple mappings** 打开可视化草稿。下拉菜单只浏览 profile；**Select as active** 修改配置中的 selected，不激活运行会话。可添加 profile，编辑名称后点击 **Rename**；简单映射填写键名（如 a、caps_lock、consumer:mute、button1），点击 **Update** 或 **Add mapping** 提交到草稿。**Remove** 删除对应简单行。包含 modifiers、多目标或其他高级字段的行显示只读提示，请在 JSON 中修改。

点击 **Use edited JSON** 才把草稿写入文档：会移除注释、重排格式，但保留未编辑的 JSON 值、未知字段与复杂规则；随后必须重新 Validate。若原始文本已变化，旧草稿会拒绝应用，请关闭草稿后重新打开。**Close visual editor** 对已修改草稿要求确认丢弃；打开文件和关闭窗口也检查草稿修改。未点击 Rename/Update/Add 的输入框内容只是待提交输入。**Save validated copy** 保存当前文档，未应用的草稿不会进入副本。

可视化界面上限为 128 个 profile、每 profile 4096 条简单映射，超过时保留文本编辑功能，拒绝可视化加载而不截断数据。参见 [ADR-0223](ADR-0223-visual-profiles-and-simple-mappings.md) 与[验证记录](validation/linux-2026-10-15-visual-ui.json)。

可视化草稿的 **Complex rules** 支持勾选启用状态、↑/↓排序、确认删除及 **Edit JSON** 单规则编辑。点击 **Commit rule JSON** 将编辑存入草稿，或明确 **Discard rule JSON edits**；有未提交修改时不能切换 profile 或应用/关闭旧草稿。完成后仍需 Use edited JSON → Validate → Save validated copy。

展开 **Import Karabiner rule asset**，输入规则库 JSON/JSONC 或单规则路径并点击 **Load rule asset**。默认不勾选任何规则；明确勾选后 **Append selected rules to draft**，完整规则按原顺序追加到当前 profile。标题、maintainers 等资产包装字段不复制；规则内部的未知字段完整保留。单规则 enabled:false 保持禁用，与上游导入时清除该状态的行为有意不同。导入不执行 JavaScript 或用户命令；不支持规则在后续 Validate 显示诊断并阻止保存已验证副本。规则列表与资产各最多4096条。上游样本中的description_notes说明字段和无to抑制规则现已支持；未知功能字段仍会触发诊断。不能据“导入成功”宣称运行会话已应用。参见 [ADR-0224](ADR-0224-complex-rule-drafts-and-asset-import.md)。

复杂 basic 规则可省略 to，表示抑制匹配按键，等同 to:[]；简单映射仍要求非空目标。规则层 description_notes 为字符串数组，保留在保存的 JSON 中。此支持经过完整离线编译、冷准备和两种执行模式测试，仍不代表真实系统设备验收。参见 [ADR-0225](ADR-0225-empty-basic-suppression-and-rule-notes.md)。

设备专属 simple 配置现在可在 Visual editor 的 **Configured devices** 中编辑；高级字段仍使用原始 JSON。Linux离线校验的KW-DEVICE-001提示类型上下文依赖；真实会话冷open已接可选udev静态类型provider，其是否可用必须由实际系统验证。当前沙箱root-trust拒绝，不把离线校验显示为“设备类型已验证”。Windows Hook仍不支持逐设备映射；已连接设备发现与运行时设备状态继续开发。

应用与输入源条件可使用UTF-8文字正则，例如 file_paths 中的 `编辑器\\.exe$`（JSON中的反斜线需按JSON规则转义）。匹配采用上游std::regex<char>字节语义：`^.$`不匹配两字节é，`^..$`可匹配；重复完整文字用分组，如 `(?:编辑器)+`，不是Unicode scalar匹配。JSON自身的\uXXXX解码与正则\\uXXXX转义不同；后者仍拒绝。缺失/过期/含CRLF的provider值保持Unknown。离线通过不证明平台已有相应输入源provider，参见 [ADR-0228](ADR-0228-utf8-byte-pattern-conditions.md)。

## 已运行 host 的状态与停止

展开 **Local session status and stop**，填写原host的 `--control-socket` 路径并 **Query host**；socket需在当前用户拥有的0700目录中，host不默认启用端点。之后每200ms查询，显示最后观察的endpoint/age、public session、phase、真实进度、已观察generation、退出及worker恢复事实。GUI不因此创建worker或抓取输入。

**Request stop → Confirm stop request** 只针对当时已查询且不超过2秒的endpoint/session；变化后需重新查询。StopAccepted表示请求交付，继续观察到Exited才能确认进程退出，recovery_confirmed独立于exit code。EOF/超时/端点消失不能当作停止完成。Disconnect停止后续观察，已发送请求可能完成；关闭GUI不关闭独立host。

Linux状态/停止在真实Unix socket、实际CLI、实际host/无输入worker和GUIclient中通过8个native cases，本次已重跑通过。没有运行真实桌面点击或输入捕获；Windows GUI接线尚未原生编译运行，Start/Resume/Apply、设备与Event Viewer继续开发。完整命令、权限边界与wire schema见 [本地会话控制](SESSION_CONTROL.zh-CN.md)。

Windows Settings 的同一控制区显示 **Windows host endpoint**，粘贴所选 host 冷输出的完整 `control_endpoint` 值（无需单独填写 PID/FILETIME），再 **Query host**。host 需显式选择 `--control-pipe`；CLI/描述符格式见 [Windows说明](WINDOWS_USAGE.zh-CN.md)。GUI源码使用同一有界 parser、pinned client 和消息事务，每帧推进一步，不在界面等待循环或自动启动 host。冷 metadata/SDK 查询仍同步，不能保证无延迟。

切换 endpoint、**Disconnect observer** 或连接失败会撤销可停止资格与二次确认。旧 pending 请求仍到有界结束，但其结果不会使 Stop 重新可用；已提交的 Stop 仍可能生效。最后观察会保留来源和 age，仅供查看；重新 Query 接受新结果后才能再次申请停止。Windows native factory/界面显示/实际 host 链仍待原生验证，当前通过的是 Linux scripted driver 与Unix socket回归。见 [ADR-0236](ADR-0236-windows-settings-status-stop.md)。

## 配置设备的专属映射

在 visual editor 的 **Configured devices** 选择 **Edit device** 或 **Add configured device**。填写 vendor/product ID（十进制或 `0x` 十六进制）及 Keyboard/Pointing device 两个精确匹配标志，再编辑该设备的 simple 映射。缺省 ID/type 沿配置语义为0/false，界面不会猜测真实设备类型。

**Commit device draft** 先完整验证本地设备buffer，再写入visual draft；**Use edited JSON** 才更新文档，之后仍需 **Validate** 和保存新副本。未提交设备字段会阻止切换profile或应用draft，并参与未保存提示；可用 **Discard device edits** 放弃。删除整个device需二次确认，已有高级映射、未知/硬件字段和其他profile不会因普通编辑而丢弃。

这里只显示配置中的selector，不是已连接设备列表，也不直接启用映射。设备最多256、同profile全局和设备simple合计4096；其他完整compiler预算仍由Validate检查。Windows Hook目前不支持逐设备身份，Linux离线通过仍带类型provider依赖，实际硬件/LED/Fn/ignore设置均未由这个面板验证。见 [ADR-0238](ADR-0238-configured-device-drafts.md)。

## 尚未提交的简单映射输入

全局simple行的 **Update**、profile名的 **Rename**、新行的 **Add mapping** 都会先处理各自输入buffer。输入尚未完成时会计入未保存状态，并阻止切换profile、结构删除和 **Use edited JSON**；这些完成按钮仍可使用，不会因pending状态被禁用。

**Discard row input** 放弃单行输入；**Discard simple inputs** 放弃未提交的全局行、名称和新行输入，保留已提交draft及独立的规则/设备buffer。无效按键或过期raw值会拒绝Update/Rename且保留输入，不会覆盖其他编辑操作已改变的值。新增mapping只有成功后重置输入。最终仍需Use edited JSON、Validate及保存新副本；这不表示运行时已应用。见 [ADR-0239](ADR-0239-global-visual-buffer-fences.md)。

Linux只读元数据CLI现可用 `keyweave-host list-linux-devices`；它与 **Configured devices** 分离，不自动将节点加入抓取或运行上下文。GUI后台刷新/选择仍待接入；类型null、partial和capability诊断不能当设备已就绪。命令边界和当前无`/dev/input`限制见 [Linux说明](LINUX_USAGE.zh-CN.md)。
