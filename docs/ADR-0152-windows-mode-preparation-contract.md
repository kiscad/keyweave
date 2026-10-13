# ADR-0152：Windows 固定执行模式的冷准备与 argv/reload 合约

Windows runtime 定义 Legacy/StagedKeyboard，PreparedConfig 保存 exact bytes/snapshot/mode，默认路径保持 Legacy；staged 预备使用实际 staged preflight，device-condition/native output 约束不放宽。worker extra args 有显式 mode-aware builder，模式位于 identity 后、config-handle 前，严格解析兼容旧 9/11 参数、允许新 11/13 参数并拒重复/未知/错误次序。

WindowsStaged reload target 冷编译保存 candidate 模式；sealed SessionActor 必须匹配其固定 mode，Legacy Engine 不接受 staged candidate。模式不可经 activation 更改。

本阶段不启动真实 Hook/SendInput；原生 capture factory/worker 入口随后接入，并保独立启动重验/监督/退出事实。参数不能在未接入的入口中被忽略后当作正确模式运行。

4 个新 tests、工作区 1387 用例/Clippy/fmt/source60 通过。actual shared SessionActor 的 staged candidate 只在同模式接受，Legacy candidate 不能切 staged actor；backend fixture 禁止任何 output。Windows-only entry 对 staged argv 在 backend/capture 之前返回 UnsupportedPlatform，避免暂未接入工厂时静默走 Legacy；本轮未做 Windows 平台编译，所以这不是该入口的平台验证。

下一步原生 install/new factory 与 host CLI 必须使用 PreparedConfig.mode，worker 独立准备也须沿该 mode；四句柄 sealed image/progress/桌面/neutral/权限/卸钩恢复/退出检查一律保留。仅源代码拒绝 staged 不代表 full objective 已完成。
