# ADR-0077：真实 Linux worker 的参数与冷配置入口

日期：2026-10-10。状态：真实 main 冷准备已接入；consumer-ready 与捕获启动尚未完成。

## 参数合约

新增独立标记 `--keyweave-linux-worker`，不套用 Windows 的 tag/identity/config path 参数。固定顺序为：marker、parent、run、`--uinput PATH`、`--session-provider none`，再接 1–64 对 `--input PATH`。

解析最多读取 136 个参数。parent/run 使用原 canonical positive decimal 校验，parent 必须可表示为 u32；所有路径保留 OsString，必须是 host-absolute，不执行 canonicalize 或其他文件查询。拒绝重复输入路径、输入与输出路径相同、未知 switch/provider、缺失字段和超限数量；保留输入顺序。最终实际打开/Probe 时仍须验证 canonical syspath、设备类型、能力和身份，不能把 lexical 路径检查当作这些证明。

Debug 仅提供输入数量与 provider，不输出路径或完整 argv。`none` 是显式无 provider，不是 readiness grant，未来也必须在这种选择下禁止打开输入/输出设备。其余 provider 尚未实现，不能接受并假装已经准备好。

## 真实 main

识别 Linux 专用格式后，首先解析参数；随后从实际 main claim ReportingStartup，核验父保护及三协议管道，比较 startup 的 parent/run 与 argv。只使用 `claim_config` 返回的封存映像，读取位置字节；不提供用户配置路径 fallback。

映像通过 Linux capabilities、generation 1、完整无损编译、最大受支持 DeviceSpec 和 Engine 的冷准备校验。JSON/UTF-8/必要功能有损等失败返回 Config；启动、FD 或身份不符返回 Admission；参数失败返回 Arguments。所有诊断仍不写二进制标准流。

有效冷配置到达 `session-provider=none` 后返回 UnsupportedPlatform，绝不打开设备，不能将该结果描述为成功启动映射。此时没有 Runtime，也不生成虚假的 TerminationReport。原继承 FD 和协议 stdio 到真实退出才关闭；副本 Drop 不作为完整资源释放证明。

同时修复非 Windows finish：保留 u8 范围的实际 scalar，不再把 Linux 冷失败统一覆盖成 UnsupportedPlatform。更大的 Windows terminal mask 不截断，在非 Windows fallback 为 UnsupportedPlatform。原 Windows 入口与旧 Linux 上的 Windows 格式 unsupported 路径保持。

## 验证与后续

新增 4 项纯参数测试：顺序与 Debug、字段/重复/provider 拒绝、无限 iterator 的有限读取、Unix 非 Unicode 路径。新增 2 项真实 exe 测试：直接调用不能绕过 guard、provider 伪 readiness 拒绝；owned Supervisor/封存 FD 交付非法 JSON、非法 UTF-8、run mismatch 和有效但无 provider 配置，观察实际 66/65/78 退出与终止管道 Missing。

有效配置测试专门选择 none，两个设备路径都不存在。禁止把该测试改成 capture-enabled provider；以后实际捕获必须有独立授权与平台验收。Missing 不是 clean/global recovery 证明，也不能由 scalar 0 替代完整终止事实。

固定 Rust 1.97.1：Linux workspace 800 tests、all-targets Clippy、fmt、60 个来源 hash 通过。[验证记录](validation/linux-2026-10-10-worker-cold-entry.json) 保存源码 hash 与实际 exe 检查。没有新增 FFI/ABI，不重复执行 C oracle；本轮没有 evdev/uinput、Hook/SendInput 或驱动操作。Windows 原生验证未执行，新增 portable 参数代码和此前 Windows provider/入口都须补验。

后续必须实现真实 consumer-ready provider、output create/discovery、输入 open/全组 neutral/grab、Runtime 移交、受控进度循环与终止交付，以及 Linux host。duration/simultaneous、hotplug/add、热配置、GUI、打包和 Rust driver 等完整原目标继续保留，不能以冷入口替代它们。
