# ADR-0150：Linux host/worker 固定执行模式交付

LinuxArguments 增加严格可选 --execution-mode legacy|staged-keyboard（固定在 provider/VT 后、inputs 前），旧 argv 默认 Legacy；builder staged 必须输出参数。限制随固定两参数调整，输入仍最多 64，拒 unknown/duplicate/reordered/non-UTF8/缺值。host 冷准备使用同一 mode，sealed bytes 原样传递，worker 按 argv mode 独立编译/revalidate 后再走同 mode bootstrap。None 仍永久禁止设备打开。

Host run-linux/check-linux 提供显式模式，默认 Legacy。worker reload target 从准备后的固定 mode 选择，不能以配置 hot reload 切换执行模式。console worker transaction 用包含 snapshot/spec/mode 的 plain data bundle，bootstrap 仍独立重验所有准入，bundle 不是 readiness token。

只运行离线/mocked/None provider 无输入验收，禁止有效 console 捕获测试；实际键盘/uinput、Windows 模式接入/平台测试仍待验证。

4 个新测试与真实 main 的 2 个追加 cases、工作区 1378 用例/Clippy/fmt/source60 通过。相同 timed bytes 的 Legacy worker Config 失败、staged None worker UnsupportedPlatform 真实退出，无 Runtime terminal 报告；None 的所有设备打开永久禁令保持。host 只运行 check-linux 离线检查与封存准备，没有启动有效 Console 捕获。

Linux 显式 staged 模式现在可通过真实启动代码到 mode-aware console bootstrap；这不证明当前主机能提供 VT/grab/consumer-ready，也不证明真实输出/时序。跨平台 host 通过 worker re-export 引用模式类型，保 Linux crate 的 target-only direct dependency。默认 Legacy gate 保持，文档应区别 code reachability、mock/None 证明和真实设备验收。
