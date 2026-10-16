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

Mixed staged 规则还支持 `from.simultaneous` 中组合键盘与鼠标按钮，首项可为 `key_code` 或 `pointing_button`；Windows 每个按钮成员仍限button1..5。普通屏幕移动和零滚轮不取消 `to_if_alone`，非零横向/纵向滚轮会取消。以上由真实配置编译与staged模型测试验证，仍需原生输入验收。

## 公开 Status/Stop 开发接口（待 Windows 原生验收）

源码新增显式 `run-windows --control-pipe`，只有选择该参数才开本地公开端点。它要求 `\\.\pipe\keyweave-control-` 后跟 32 个小写十六进制字符；同名实例已存在则启动失败，不自动附着。沿用前文 run 的其他必需参数。冷阶段输出：

```text
control_endpoint=<pipe>|<host PID>|<完整 creation FILETIME> public_session=<独立 public nonce>
```

复制完整 `control_endpoint` 值并在 shell 中加引号，可调用：

```powershell
.\keyweave-host.exe status-windows --endpoint '<完整 control_endpoint 值>'
.\keyweave-host.exe stop-windows --endpoint '<完整 control_endpoint 值>' --session <已观察的 public_session>
```

这些客户端命令不创建 worker 或开启输入。StopAccepted 仅表示意图已排队，不代表实际退出或输入恢复；状态中的 Exited 只来自 host 实际观察。当前 recovery_confirmed 始终 false，cold metadata/Starting 也不代表输入已就绪。host 实际退出后只继续最多 500ms 最终状态服务，端点消失不能当作退出/恢复证明。

该版本的 Windows host/CLI 专属分支尚未原生编译执行。Linux 测试覆盖 parser、公共状态与 unsupported；transport 的 API mock 也不代替实际 Windows pipe/同 owned child 退出验证。GUI 仍待接入 Windows transport；开发边界与验收步骤见 [ADR-0235](ADR-0235-windows-host-public-control.md)。
