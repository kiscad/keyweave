# ADR-0041：通过 WSL 调用 Windows 工具链与真实 MSG ABI

问题：Linux/mock tests 尚未证明 Windows 生产分支或真实 SDK。用户提供 Windows Rust 安装；WSL interop 在受限沙箱内 socket EPERM，沙箱外可读出 Windows Rust 1.99.0、VS Build Tools 与 SDK。初次真实 SDK oracle 发现 MSG 的 lPrivate 断言错误。

选择：保留项目固定 Rust 1.97.1；先显式用现有 +stable（本机 1.99.0）编译，结果不冒充固定版本门槛。编译/链接与 mock tests 不调用真实 Hook/SendInput。产物与依赖用独立临时目录，不改默认工具链。真实 SDK 的 MSG 仅在 `_MAC` 下包含 lPrivate，目标 Windows 非 `_MAC`，Rust 删除该字段；x64 size=48（尾部对齐 padding），x86 size=28。同步更正 Rust 断言与 C11 compile-only oracle，独立执行 x64/x86 SDK 检查。

证据：本机 Windows SDK 10.0.26100.0 的 um/WinUser.h tagMSG 声明；MSVC 19.44.35229.0（工具目录 14.44.35207），首个 x64 /Zs oracle 编译报 lPrivate 非 MSG 成员。参考 [Microsoft MSG 文档](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msg)；公开文档展示的保留字段不能覆盖安装 SDK 的条件声明。

边界：本机仅安装 x86_64-pc-windows-msvc stable target，i686 Rust target 与固定版本尚未证明；SDK x86 compile 不等于 Rust x86 编译/运行。WSL 文件共享上的 Cargo SQLite cache 锁警告不能伪装为编译成功，需以最终退出/测试结果记录；后续优先使用 Windows 本地临时 cache/target。没有硬件捕获/注入、driver 或完整会话验收；独立监督/重复/碰撞/可执行 host 等工作继续保留。


结果：改 Windows-local 源快照/cache/target 后，Rust1.99.0 x64 workspace432 tests、native_link 与 clippy -D warnings通过；x64/x86 SDK实际通过。portable Linux mocks 改host-absolute identity，保持所有生产检查与故障测试；新Clippy切片lint采用as_chunks，Linux1.97.1验证通过。tools/validate_windows.ps1 已在 Windows PowerShell5实际执行 x64完整与x86 SDKOnly；输出源码hash与日志，94源文件hash匹配工作区。版本、命令范围和边界见 docs/validation/windows-2026-10-09.json；Windows固定1.97.1/i686仍未执行。
