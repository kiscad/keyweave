# Windows 使用说明

当前 Windows 键盘入口已扩展显式的实验性 mixed 范围。缺省仍为 `keyboard`；只有 `--capture-scope staged-mixed` 且 `--execution-mode staged-keyboard` 才选择键盘与鼠标按钮/滚轮链路。不会根据配置内容自动开启鼠标捕获。

先进行离线配置检查（也可在 Linux/WSL2 运行，无需设备或 helper）：

```text
keyweave-host check-windows --config config.json --execution-mode staged-keyboard --capture-scope staged-mixed
```

成功会显示 `scope=StagedMixed` 和 `device readiness unverified`。支持五个鼠标按钮的键盘↔按钮映射、滚轮沿 staged 队列原 delta 转发，以及按住时保留旧规则的热更新基础。按钮6及以上、设备专属条件和不支持的输出会在冷编译时拒绝。屏幕位置只作为活动通知并透传，尚不支持移动重映射或 mouse_key。

在实际 Windows 环境中，显式启动命令如下（需要匹配版本的 host/worker；路径按实际安装位置调整）：

```powershell
.\keyweave-host.exe run-windows --config C:\KeyWeave\config.json --identity canonical-scan --execution-mode staged-keyboard --capture-scope staged-mixed
```

可以用 `--worker` 选择绝对路径的 worker，用 `--artifact-directory` 指定已有本地目录。host 将同一已验证配置的原始字节封存，经原有 owned Job/句柄事务交付；worker mixed 入口强制继承配置句柄，不能重读用户路径。Enter 或 Ctrl+C 请求停止。配置监视/GUI、完整 driver 路线和安装包装尚待完成。

本轮测试是在 Linux 下验证 CLI、配置编译、参数范围绑定和既有 Win32 mock 生命周期；没有启动实际 Windows Hook 或注入输入。Windows 原生构建、SDK/Job/Hook/鼠标实机验收仍需按 [验证说明](WINDOWS_VALIDATION.zh-CN.md) 补齐，离线配置接受不表示系统可捕获或应用已收到输出。
