# ADR-0239：全局映射与 profile 名称输入不丢失

既有全局 simple 行使用独立文本buffer，点击Update前未参与pending；profile名称与新增mapping输入也存在相同边界。修复为所有simple/name/new inputs参与VisualPane.pending/Settings unsaved，阻止profile切换、Use edited JSON与隐式关闭，不以只检查已提交draft代替输入状态。

行buffer同时保存原始Value与canonical显示baseline；Update前核对原始值，过期/已移除/advanced/无效按键均拒绝且保留输入，失败不改draft。名称同样核对expected optional name。单行Discard与总Discard simple inputs明确放弃buffer；新增mapping仅成功后重置输入。结构删除需先完成全部pending buffer，避免索引移位时清空别行未提交文本。Update/Rename/Add/Discard仍可在pending时操作，不因禁用profile区域陷入死锁。

仅更改GUI草稿，不启用runtime或覆盖文件；完整source fence、unknown/advanced JSON保留、设备/规则pending与离线backend gates保持。无输入/桌面/Windows原生验收声明，完整目标继续。

补充：profile名称、全局行、新mapping、设备selector/行文本使用固定widget ID，避免pending提示插入改变自动ID并丢焦点。没有待提交文本的cache可刷新到显式改变后的draft；有pending时绝不自动rebase到新expected值。全局profile浏览只改变显示，不冒称draft已改。

验证：新增6项覆盖Settings unsaved/Apply/browse拒绝、无效与过期/已移除行不覆盖、名称fence与新mapping保留/重置、多行独立commit与结构删除、headless重绘不自动提交，以及egui Event::Text在名称/全局行/新mapping/设备vendor上连续输入两帧保持焦点。Linux全量2010通过，8已有native默认ignored；格式/Clippy/源码62/Linux ABI/X11 SDK/regex oracle通过。合成UI事件不是OS输入注入，未创建实际桌面窗口或执行Windows原生。见 [验证记录](validation/linux-2026-10-16-visual-input-fences.json)。
