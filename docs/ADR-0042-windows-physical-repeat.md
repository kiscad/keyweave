# ADR-0042：Windows 按来源转发物理重复

Windows Hook 已按 saved native scan 将后续 physical down 归一为 Repeat，但普通 Engine 不生成重复输出；SendInput 单次 down 没有本项目已验证的自动重复保证。不能直接重复最后一个全局输出键，也不能重新执行映射数组来补重复。

选择沿用实际物理 Repeat 的节奏，不新增软件 repeat timer。Engine 提供只读 physical_repeat_target(device, source_key)，按仍活动的原 binding 查当前保留主动作的末端 leaf；simple/complex 展开与 held replacement 都保存明确 repeat_child，不能扫描任意 child 或回退到 modifier scope。主 action repeat:false、已结束 tap、variable、suppressed、modifier/非键盘输出、旧代际与已释放来源不产生目标；还要求对应 SourceToken 仍拥有且可见、逻辑键已 emitted。查询不创建 owner、不重匹配规则、不修改变量。热更新仍读旧 binding，held deadline 先于当刻 Repeat 更新目标。

SessionActor 在既有 due→Engine Repeat→due 顺序之后查来源目标，并要求 OutputQueue 的 desired/submitted 都仍含该键，然后追加一条 Repeat，经原有有界 pump 同步提交。Repeat 编码为带 marker 的 scan down，无额外 up；desired/submitted owner 状态不增加。每个回调仍共用 MAX_WORK，短返回/拒绝停止 actor，外层停 Hook 后只释放 accepted ledger 的原 down；未知接受量保持 quarantine，不因为“只是重复”猜零接受或清账本。Linux Kernel repeat 路径与普通 Engine/CLI Repeat 输出不变，只有 Windows actor 调用此查询。

重复采用当下输出修饰状态，如原生持续按住的键；不重新建立首次 shortcut 的临时修饰范围，也不重播宏前缀、alone/up/delayed/变量动作。比如 a→Ctrl+b 首次发送 Ctrl down/b down；既有 lazy-up 语义将 Ctrl up 延至下一 down 或设备 neutral。此时 a 的物理 Repeat 仅发送 b down，仍受已 emitted 的 Ctrl 影响。另按 Shift 会先同步 Ctrl up/Shift down，此后的重复受当下 Shift 影响。这个选择保持当前引擎与 Linux 持键重复的状态语义，不能声称重复整条快捷键宏。

依据：[Microsoft Keyboard Input Overview](https://learn.microsoft.com/en-us/windows/win32/inputdev/about-keyboard-input) 描述长按产生连续 key-down 与最终单次 key-up；[KEYBDINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-keybdinput) 描述 scan-code 输入；[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput) 描述插入数量、串行插入与现有键状态干扰。根据这些接口作出的本项目策略是转发观察到的物理重复，文档不构成“SendInput 会自动重复”的证明。

边界：Hook 流没有真实 device ID，同 scan 多键盘重叠不能独立计数；所谓物理 Repeat 由保存按住状态识别，不保证可以区分同一设备硬件重复与另一键盘同 scan 新 down。源设备未产生 Repeat 时不补定时重复，也不在松键后继续补发。物理 Pass/injected 冲突协调、Hook 存活与独立监督、完整可执行会话及真机重复速率/特殊键/布局验收仍未完成。

验证：5个新engine集成与8个新Windows actor/dispatcher/pump mock tests；Linux482 tests和独立worker harness、fmt/clippy/source verifier通过，Windows1.97.1 x64/i686各445 tests、native_link、clippy与SDK oracle通过。各128文件快照hash匹配，见 [Windows结果](validation/windows-2026-10-09-repeat.json)。没有安装真实Hook或调用SendInput，真机长按行为仍待完整可执行host后验收。
