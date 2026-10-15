# ADR-0224：复杂规则草稿与 Karabiner 规则资产导入

问题：原生 GUI 仅有整文档 JSON 与 simple 表，无法从 Karabiner complex_modifications 规则库明确选择规则，也无法单独维护复杂规则顺序。

选择：复用 VisualDraft 全量 JSON AST/原文 fence。复杂规则列表支持启用/禁用、上移/下移、明确删除及单条 JSONC 编辑；单条替换保留用户给出的完整对象，不过滤未知字段。编辑器为显式提交缓冲区；有未提交修改时不允许切换 profile/规则、关闭草稿或应用旧草稿，需提交或明确丢弃。语义及 OS 能力仍交共享 Validate，不另造兼容编译器。

导入复用文件层8MiB有界读取与严格JSONC，识别规则库 {title,rules:[…]} 或单规则 {manipulators:[…]}。先展示来源、标题、规则数量、每条描述，用户明确勾选后追加至当前 profile；不替换旧规则（KeyWeave追加，区别于上游push_front_rule_to_core_configuration_profile的前插），不因不支持字段跳过规则。只复制明确选择的完整规则对象；资产包装层 title/maintainers/未知字段不属于 profile，界面明确说明不会复制，原文件不改。最多4096条；超限、重复JSON键、错误结构或选择无效在修改前拒绝。导入与规则JSON编辑不会运行 eval_js/shell/用户动作，也不会自动禁用不支持规则。编译器诊断和保存 gate 继续负责禁止有损执行。

依据本地固定上游 ec2fea8940f7254a748c052a890ee9db7113d9b1 的 src/share/complex_modifications_assets_file.hpp、tests/src/complex_modifications_assets/json/single_rule.json 和 complex_modifications/1.json；包装/单规则导入形态参考源码，不运行上游 macOS 测试。规则资产 UI 使用显式文件路径，不扫描目录、递归链接或下载规则。未提交输入与磁盘/运行时激活严格分离；没有输入捕获或注入。

上游单规则load_rule会erase enabled而启用规则；KeyWeave明确保留原enabled状态，此为避免导入意外启用的兼容差异。三个未改动上游fixture复制至tests/fixtures/upstream/complex_assets并登记hash；测试比较完整规则值，不假称运行macOS上游测试。

此切片不引入 GUI→运行会话 IPC。实际桌面/中文字体/可访问性/托盘和 Windows 原生构建仍需后续验收。与新增 GUI 内容一起加入顶层滚动区域，保证低高度窗口可到达校验/保存按钮。
