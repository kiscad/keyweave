# ADR-0223：可视化 profile 与简单映射草稿

GUI 新增显式 VisualDraft，复用严格parse_jsonc解析，保留完整JSON Value，包括未知字段/复杂规则/其他profile。可浏览profile、显式选择active、重命名/添加profile，以及编辑/增加/删除单一from→单一to简单映射。含高级字段或多to的行只显示高级标记，不能被普通编辑自动覆盖。

可视化操作仅更改草稿；Use edited JSON明确说明重写格式和注释，再将完整JSON序列化到编辑器。以原文精确比较防止旧草稿覆盖同时发生的文本编辑，超8MiB/非法结构拒绝且不改当前文档。写入编辑器使用edit而非load/replace，保留原baseline并撤销旧校验，必须重新验证后保存新副本。profile浏览不改变selected；激活按钮才将恰一profile.selected置true。

草稿未提交的修改也纳入打开文件/关闭窗口确认；丢弃草稿需要显式操作。原始磁盘文件从不覆盖，未知JSON字段不因界面不支持而删除。为有界UI限制128profiles/4096simple rows，超过则继续用原文本编辑器，不自动截断。此为离线配置编辑，不声称已切换运行会话profile或已应用generation。
