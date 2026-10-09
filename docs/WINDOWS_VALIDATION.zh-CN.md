# Windows 原生验证入口

当前 Linux 主机的 portable/mock tests 不证明 Windows FFI 可编译、链接、布局正确或能实际拦截按键。生产代码与 unit-test 的 Win32 API 替换必须分开验证。

## SDK 与编译

使用 Windows x64 主机，安装 Visual Studio C++ Build Tools 与 Windows SDK。`tools/verify_windows_abi.py` 使用 [Microsoft vswhere](https://github.com/microsoft/vswhere/wiki/Find-VC) 定位工具，不假定 VS edition/version。执行：

```text
python tools/verify_windows_abi.py --arch x64
python tools/verify_windows_abi.py --arch x86
rustup target add x86_64-pc-windows-msvc i686-pc-windows-msvc --toolchain 1.97.1
cargo test --locked --workspace --target x86_64-pc-windows-msvc
cargo test --locked --workspace --target i686-pc-windows-msvc
```

SDK oracle 为 `tools/windows_input_abi.c`，只编译 `/Zs`，不链接/执行；验证实际 SDK 的 INPUT/KEYBDINPUT/MOUSEINPUT/HARDWAREINPUT/KBDLLHOOKSTRUCT/MSG 大小、offset/alignment、固定目标 pointer width、flags/消息/wait 常量，以及 WINAPI 原型。`_WIN32_WINNT/WINVER=0x0a00` 固定 Windows 10/11 接口层：QS_ALLINPUT 包含 TOUCH/POINTER，值为 0x1cff，不沿用旧 0x04ff。

Rust `native_link` integration test 通过正常 crate 依赖构建生产模块，保留 Hook/SendInput/MsgWait 等函数地址以验证链接；只构造固定内存 buffer，不调用安装/注入。Rust native 布局断言与 SDK oracle 在同一个 x86/x64 CI job 中运行，任一 mismatch 阻止通过。workspace unit tests 在 Windows 也替换 Hook API，不进行全局捕获。

`.github/workflows/rust.yml` 已配置两种 MSVC 目标。工作流存在不等于已经执行或通过；真实结果须记录 runner、SDK/MSVC/Rust 版本、target 与 job URL。本环境无 Windows SDK/交叉 linker，只有 Linux rust-std；隔离到 `/tmp` 的 1.97.1 Windows target 安装尝试因 DNS 失败，未获得交叉编译结果。

## 真机门槛仍未完成

编译/链接/SDK 检查均不能证明输入拦截或应用接收。待完整可执行会话、neutral/碰撞/重复/紧急退出/监督接入后，按 D08 记录 Windows 11 x64、普通/管理员程序、中文布局、特殊键、Injected、睡眠/安全桌面/RDP，以及部分发送后的实际恢复。不能用上述无需安装的测试替代这组验收。

来源：[Microsoft WinUser.h](https://github.com/microsoft/win32metadata/blob/main/generation/WinSDK/RecompiledIdlHeaders/um/WinUser.h)、[INPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-input)、[KBDLLHOOKSTRUCT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-kbdllhookstruct)、[MSG](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-msg)、[MsgWaitForMultipleObjectsEx](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-msgwaitformultipleobjectsex)。
