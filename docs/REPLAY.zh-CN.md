# 分阶段离线回放

`keyweavectl` 的 validate、import-karabiner 和 replay 均接受 `--execution-mode legacy|staged-keyboard`，默认 Legacy。Complex 组合键与持续时间规则需要显式 staged-keyboard。所有命令保持离线；import 写副本及报告，不激活配置。

```sh
keyweavectl validate --execution-mode staged-keyboard --config examples/staged-chord.json --format json
keyweavectl replay --execution-mode staged-keyboard --fixture tests/fixtures/staged-chord.json --assert
keyweavectl replay --execution-mode staged-keyboard --fixture tests/fixtures/staged-duration-reload.json --assert
```

回放使用同一个 Physical/Simple/Complex/Output 模型。时间以整数纳秒表示：普通输入前执行到该时刻为止的期限，实际 poll 采用下一期限时刻；Reset、DeviceRemoved、ContextChanged 控制先交付，再执行同刻剩余工作。组合键阈值端点包含在窗口内，窗口超时在端点加一纳秒才可确认。此虚拟调度不模拟原生线程延迟或应用接收。

| 格式 | 输入 | 使用范围 |
|---|---|---|
| schema_version 1 | 原有 inputs 数组 | Legacy 或显式 staged；输入格式不改变 |
| schema_version 2 | steps 数组，禁止非空 inputs | 仅显式 staged；支持时间推进与配置替换 |

schema 2 的每个 step 使用严格 `type` 字段；其他未知字段拒绝：

```json
{"type":"input","input":{"seq":1,"at_ns":100,"device":{"id":1,"generation":1},"event":{"type":"key","key":"a","edge":"down"}}}
```

`advance` 格式为 `{"type":"advance","at_ns":50000101}`。它执行至指定虚拟时间，不分配 input seq、timer token 或额外物理来源。`activate` 必须包含 `at_ns`、严格递增的 `generation` 和完整 `config`，例如分阶段持续时间 fixture 中的第二代配置；到期旧工作先执行，新配置冷编译后交给实际 controller，已按下的旧绑定继续保存旧 Arc。所有 steps 时间不倒退；input seq 由 controller 核验。不能把 Legacy TimerFired/CancelDelayed token 输入用于 staged。

回放结束不自动推进到未来，也不自动 Reset。未完成任务返回真实 state 与 next_deadline；需要观察晚抬键或超时的 fixture 必须显式添加 advance/input。`--assert` 要求 expected 输出，可选 expected_state、expected_variables、expected_pointers。任何差异返回非零，模型没有执行的输出不能列入 expected。

输出 JSON 中 outputs 是键盘意图，不是操作系统接受证明；forwarded_pointers 是转发 receipt，保存 delivery/original 时间、设备和 motion。每条 pointer 的 after_keyboard_outputs 表示之前已有多少键盘输出，以保留跨种类先后关系。repeat_targets 只是原 Repeat 输入对应的实际 saved leaf 查询，不重复发送键盘 Down；Windows 是否重复和 Linux 内核重复策略留给后端。变量断言读取实际 Output 生成平面。

输入文件最多 8MiB、100000 个输入/step；最多 100000 条键盘输出和 100000 条 pointer receipts，以及1000000次驱动工作。超过上限、非法 schema、时钟或身份、无法编译的配置都明确失败，不写半份成功报告。doctor 区分后端代码存在与 native_acceptance/hardware_validated，不能用回放成功替代真实设备验收。

相关 fixture：staged-chord、staged-chord-endpoint、staged-chord-timeout、staged-duration-reload、staged-chord-remove、staged-pointer-variable。既有上游 fixture 不修改。见 [ADR-0174](ADR-0174-staged-cli-replay.md) 和 [状态](STATUS.md)。
