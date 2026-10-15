# Linux 当前使用与验收边界

## Mixed 配置离线检查

在启动前验证同一声明的输出能力，无需 worker、helper、provider 或输入设备：

```sh
keyweave-host check-linux --config /path/config.json \
  --execution-mode staged-keyboard --relative-axes 0,1,6,8,11,12 \
  --pointing-buttons 272,273,274
```

不传轴/按钮则保留键盘检查；传轴选择 `StagedRelative`，必须 staged 模式；按钮必须配 X/Y，未知/重复能力拒绝。配置中目标按钮未声明时拒绝，即使另一个按钮已启用。成功输出包含 `scope=StagedRelative` 和 `device readiness unverified`，仅表示配置编译和声明能力覆盖，不表示设备或 X server 可用。

## 实验性 X11 mixed 会话

`run-linux --session-provider x11` 已连接 X11 启动、consumer lease、停止清理与配置监视。必须显式传入 `--execution-mode staged-keyboard`、绝对 `--x11-probe` 路径和 `--relative-axes`；不能携带 `--vt`。helper 必须名为 `keyweave-x11-probe`，位于实际 worker 同目录，并通过配套文件权限校验。

轴代码：`0` X、`1` Y、`6` 水平滚轮、`8` 垂直滚轮、`11` 垂直高分辨率滚轮、`12` 水平高分辨率滚轮。逗号分隔，必须来自实际输入设备能力；未知或重复值拒绝。可选 `--pointing-buttons 272,273,274` 对应左/右/中按钮，可用范围 272..279；省略表示无按钮输出，启用按钮要求 X/Y 两轴。配置与实际输出能力会在启动前匹配。

参数示例（路径和能力需替换为实际值）：

```sh
/path/keyweave-host run-linux --config /path/config.json \
  --worker /path/keyweave-worker --session-provider x11 \
  --execution-mode staged-keyboard --x11-probe /path/keyweave-x11-probe \
  --relative-axes 0,1,6,8,11,12 --pointing-buttons 272,273,274 \
  --input /dev/input/event1 --input /dev/input/event2 --watch-config
```

本轮验证覆盖离线 CLI 和 worker 参数转换，尚无真实 X server/driver、consumer delivery 或硬件捕获验收。`check-linux` 已支持同名 relative/button 参数的 mixed 配置离线检查，仍不能证明设备或 consumer 就绪。完整限制见 ADR-0205/0206 与 STATUS。

console host 支持显式 `--watch-config`，后台读取变化的正文、由 worker 冷准备并确认实际激活；默认不启用。读取失败或拒绝保留旧配置，结果在停止后的汇总中显示。当前实际捕获与监听入口仍需真机验收；None 永久拒绝设备操作且不能启用监听。见 [ADR-0101](ADR-0101-linux-watch-command.md)。

Linux 已提供实际 `keyweave-host run-linux` 入口，连接冷配置校验、密封字节、独立 worker 进程组、父死亡保护、信号/进度监督与 KWLT 终止报告。当前原生消费者只支持显式文本 VT；图形桌面、Wayland/X11 与普通 WSL2 终端尚不能获得该消费者授权。

## 构建与离线校验

```sh
RUSTUP_TOOLCHAIN=stable cargo build --locked --offline -p keyweave-host -p keyweave-worker
target/debug/keyweave-host check-linux --config examples/karabiner-basic.json
target/debug/keyweave-host run-linux --help
```

发布布局默认把 `keyweave-host` 与 `keyweave-worker` 放在同一 canonical 目录；Linux worker 无 `.exe` 后缀。开发时可用 `--worker /absolute/path/keyweave-worker` 选定可信映像。定位不使用 PATH/shell；regular 文件和执行位检查不构成映像认证，也不能代替最终 spawn 的权限/挂载检查。

## 永久无设备模式

以下模式只验证 host/worker 冷启动协议，没有映射能力。输入路径仍接受严格 lexical 校验，但不会查找或打开设备；不得加入 `--vt`：

```sh
target/debug/keyweave-host run-linux \
  --config examples/karabiner-basic.json \
  --session-provider none \
  --uinput /keyweave-absent-output \
  --input /keyweave-absent-input
```

有效配置预期 worker 返回78，host 返回1，报告 `terminal=missing`、`recovery_confirmed=false`。这不代表映射启动成功；None 是永久拒绝设备操作的契约，不能日后变成默认消费者或把有效配置自动测试改成 console。完整真实 host→worker 的 None 验证见 [记录](validation/linux-2026-10-10-run-command.json)。

## 显式文本控制台模式

这是实验入口。下例仅展示参数，应在实际专用 Linux 文本控制台上，确认选定 VT 和设备权限、完成真机测试授权后手动运行。不能在当前 WSL2 会话中当作有效消费者运行：

```sh
target/debug/keyweave-host run-linux \
  --config /absolute/path/karabiner.json \
  --session-provider console --vt /dev/tty2 \
  --uinput /dev/uinput \
  --input /dev/input/event4 \
  --input /dev/input/event7
```

设备和 VT 数字必须来自实际机器。VT 只能使用 literal `/dev/tty1` 至 `/dev/tty63`；选择当前活动 text VT，并从具有该控制终端的前台 host 组启动。程序不切 VT、不创建 session、不提权、不修改系统设备权限。keyboard translation 模式、session/前台组与消费者检查见 [ADR-0078](ADR-0078-linux-text-console-readiness.md)。最多64个不同输入路径，均须 absolute，且与 uinput 路径不同；该参数检查不证明它们都是适用设备。

worker 在设备操作前核验父死亡保护、独立组与 unblocked stop signals；以有限 bootstrap 创建输出、发现 owned 节点、逐个打开输入，等待消费者/中立状态，整组抓取后再检查。首次 dispatch 前和活动期间更新消费者证据。未知、过期、切换或故障会停止并进入有限恢复，不自动重新 arm。

## 配置文件监听

在上面的实际 console 命令末尾增加：

```sh
--watch-config --watch-interval-ms 1000
```

周期兜底检查间隔100～60000毫秒，启用时默认1000；指定间隔必须同时启用监听。源配置可使用相对路径或普通symlink，程序在冷准备时固定其绝对文件名，后台按该文件名读取完整变化正文。建议用同目录临时文件加rename保存，读取中检测到变化会保留旧配置后再检查。

保存新配置触发下一次检查；同一已接受/已拒绝正文不会反复提交，恢复已接受正文无需新增代号。有效Accepted才更新配置基线，已按下键仍按原binding完成repeat/up。无效正文/权限/文件删除会保留旧配置，不改用默认配置。目录中的选定文件写入/替换/删除提示会在200ms安静期后开始检查，持续变化最多合并到1s调度；任务或事务忙时仍须等待。通知不可用或丢失时保留周期正文读取，原路径parent/symlink目标变化也依赖兜底；当前没有独立手动reload命令。见 [ADR-0102](ADR-0102-linux-directory-notice-debounce.md)。

活动监督循环不打印状态；退出后显示 `reload_submitted/accepted/rejected/cancelled`、`config_read_failed`、最近确认的 `reload_confirmed_generation`、`config_watch_fault` 、`config_source_reaped` 和缓存 `config_notify_error`。无确认时generation为None；Submitted不能当激活成功。文件任务未能在退出后的一次100ms窗口回收，host返回1并显示false；任务回收不能证明键盘恢复或worker退出。SIGHUP仍请求Stop。

## 停机与报告

SIGINT（Ctrl+C）、SIGTERM、SIGHUP 由 host 主线程 signalfd 接收，首事件请求一次协作 Stop。worker 在独立组中保留原 session，避免同前台组信号绕过 Stop；实际终端 Ctrl+C 的完整硬件交付尚未验收。不要把直接向 worker PID/其组发送信号当作协作停止。

正常运行 host 独立监督实际 worker poll 的进度。停止请求不续租或延长原截止时间；超时可强制终止 owned worker。只有实际退出后才有界收集终止 frame。报告分别显示 `worker_exit`、`worker_signal`、监督 `reason`、`stop_signal`、`terminal`、恢复 facts、host fault 与信号掩码关闭失败，不打印配置内容、输入事件或 run ID。

host 返回0要求实际 worker code0、可接受终止原因、完整恢复 facts 与无 host/关闭故障。pre-Runtime 停止没有 KWLT，当前不会被 host 自动判为已完成恢复。观察超时返回1并报告 `worker_exit=unobserved`；其 emergency Drop 的有限 kill/reap 不算已观察到退出，更不证明全局键恢复。库调用方可保留 NativeSession 继续观察同一个 child，不能因超时重新 spawn。

## 暂停时间检查

原生时钟保留 evdev 的 MONOTONIC 时间域，同时用 BOOTTIME 夹在两次 MONOTONIC 之间的采样检查连续性。检测到偏移区间不一致、倒退、过宽或调用失败会永久使本轮时钟失效，停止路径保留最后接受的时间；不自动创建新 clock 继续原授权。该检查仍有区间歧义和采样后的竞态，不能视为完整 suspend/resume 支持，见 [ADR-0088](ADR-0088-linux-clock-continuity.md)。

## 尚需完成

当前验证覆盖离线引擎、模拟输入/输出、无输入子进程和真实 None 冷链路。真实 VT、evdev 抓取/uinput 输出、按键释放、暂停/切换、Ctrl+C、suspend/resume 与系统权限/故障仍需真机验收。图形消费者、动态添加设备、LED、高级规则、跨平台热配置、GUI、包装发布与驱动仍未完成；完整状态以 [STATUS](STATUS.md) 为准。


Linux staged 规则现支持 `from.simultaneous` 组合媒体键与键盘键，例如 `[{"consumer_key_code":"mute"},{"key_code":"a"}]`。首项也可为键盘；两者在阈值内匹配后只发送所配置目标，未匹配则按原顺序回放。此能力使用已有 Linux consumer 映射与 backend gate，仍须通过现有设备、session/consumer-ready 和 neutral 准入；规则接受不是硬件捕获证明。Windows consumer_keys 仍关闭，不能套用这一规则。

## 设备专属 Simple 的当前边界

配置编译器现可读取 `profiles[].devices[].simple_modifications`：先按设备数组顺序尝试专属映射，再尝试 profile 全局 simple，随后进入 complex。设备项要求 `identifiers` 对象；与固定上游 factory 一致，用 vendor_id/product_id（缺省0）、is_keyboard/is_pointing_device（缺省false）四项匹配，location/address不参与这类profile selector；原始数据仍保留。复杂规则的 device_if 条件继续按其显式字段求值。

Linux Evdev 冷启动现已连接可选 libudev 静态类型 provider（见下节）。只有完整、可信、身份核验通过的分类记录才提供 keyboard/pointing 的明确值；provider 不可用时仍只有 vendor/product，四字段设备条件不会成立。编译通过只证明配置/引擎准入，`KW-DEVICE-001` 会提示需要可信类型事实；未知、过期或不同连接代际回退到全局 simple。Windows Hook 无可靠逐事件设备身份，继续拒绝设备专属映射。

每个设备最多沿共享4096规则预算，设备项总数最多256。ignore:true、未知设备设置、非空 Fn 映射仍拒绝；ignore:false、disable_built_in_keyboard_if_exists:false、treat_as_built_in_keyboard:false 是明确支持的无操作值。此功能不负责发现设备、打开输入节点、LED/系统键盘禁用或证明 capture ready。见 [ADR-0226](ADR-0226-device-specific-simple-mappings.md)。

## 静态 udev 类型 provider

真实 `EvdevDevice::open` 在抓取之前查询当前已打开FD的libudev设备。要求 initialized、ID_INPUT=1、至少一种已知类型标签、固定系统libudev路径及 `/run/udev/data/cMAJOR:MINOR` 记录的root/权限检查通过，并核对devnum、input父路径及前后内核身份。keyboard取ID_INPUT_KEYBOARD，pointing取MOUSE/TOUCHPAD/TOUCHSCREEN/TABLET/TABLET_PAD/POINTINGSTICK的OR；consumer、built-in等没有来源的属性保持未知。该分类是Linux udev静态分类，不等同macOS HID角色，也不证明libinput消费者已初始化。官方[类型标签说明](https://wayland.freedesktop.org/libinput/doc/latest/device-configuration-via-udev.html)作为来源。

类型只在本次打开时读取，随后作为设备实例静态元数据复制到上下文。运行中不加载库/读取udev；修改udev规则后需重新打开会话。没有可用分类记录时整个类型观察保持Unknown，不能按属性缺失补false；只有完整初始化分类范围内，未标记类别才解释为false。

只读诊断：`RUSTUP_TOOLCHAIN=stable cargo run -p keyweave-linux --example udev-provider-check --locked --offline`。它只检查库路径信任/符号可用性，不枚举/打开输入设备，也不抓取或注入。当前沙箱实际返回exit1（系统库与祖先显示UID65534，严格root边界拒绝）。未降低检查，未实际完成libudev设备查询或硬件验收；本机也缺少libudev.h，新增FFI签名按固定v257官方header审阅，尚无本地SDK C编译证明。见 [ADR-0227](ADR-0227-udev-static-device-types.md) 与[验证记录](validation/linux-2026-10-15-udev-types.json)。
