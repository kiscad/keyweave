# 项目状态

更新日期：2026-10-08。

| 项目 | 状态 | 证据 |
| --- | --- | --- |
| macOS 本地主程序与测试源码分析 | 已完成文档评估 | SOURCE_AUDIT 与 source-manifest.json |
| Windows/Linux 官方接口核验 | 已完成文档核验 | 开发指导文档中对应链接 |
| 新目录、开发指导、任务清单、AGENTS | 已创建 | 本目录 |
| 文档完整性检查 | 通过 | 59 个本地链接、48 个源码 SHA-256、JSON 语法、Markdown 围栏；上游工作区未变 |
| D00 来源固定与 ADR | 基础完成；companion 审计仍缺失 | tools/verify_sources.py、fixtures 原始复制、ADR-0001/0002 |
| D01 Rust workspace 与离线切片 | 已实现；Windows 编译待执行 | 4 crates、Cargo.lock、固定 1.97.1、82 个本机行为测试通过 |
| D02 HID 键名 | 部分实现 | 309 个标准键名/别名及 round-trip；native 转换/持久设备选择待做 |
| D03 配置编译与 CLI | 部分实现 | JSONC（含重复成员拒绝）、元数据往返、各执行层字段检查、整条禁用、validate/import/replay；高级 schema 待做；COMPATIBILITY 描述当前范围 |
| D04/C1、D05/C2 完整规则 | 部分实现，未完成 | 多动作跨阶段 down/up 生命周期、持键热更新、多来源 ownership 与展开限额；optional:any、alone/after-up、修饰快照恢复与旧快照宏预算已实现；标准 mandatory/optional/any 与左右/抽象组、lazy 消费恢复已实现；repeat:false 已实现；to modifiers 与设备 neutral 刷新已实现；设备/应用/输入源条件与有效期上下文已实现，变量也已实现，其他 conditions、timers 等字段仍拒绝；ADR-0010/0009/0008/0007；ADR-0003/0004/0006 |
| Linux / Windows 用户态后端 | 未开始 | 尚无程序 |
| Windows Rust driver 可行性原型 | 未验证 | D10 研究门槛 |
| macOS 上游测试执行 | 未执行 | 当前 Linux 环境，Apple framework 依赖 |
| 目标平台硬件、权限、故障恢复、性能 | 未执行 | 后续专用测试系统验证 |
| companion DriverKit 源码/许可证审计 | 未完成 | 本地子模块未初始化，固定版本远程读取失败 |
| 项目许可证、依赖/MSRV | 已选定 | MIT，保留 Unlicense/BSL 来源；Rust 1.97.1、MSRV 1.97、Cargo.lock |
| 公开名称可用性 | 尚未核验 | 发布前任务 |

下一任务：实现 D05 held/delayed/simultaneous 时间规则及显式 lazy/halt/hold duration，然后进入 Linux 后端。完整后端、session/GUI/驱动及发布目标保留，尚未达成全部开发目标。

本机验证：Ubuntu 24.04.4；`RUSTUP_TOOLCHAIN=stable cargo fmt --all --check`、`cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo test --workspace --offline`、basic/sequence/tap-alone/modifiers/output-shortcuts replay `--assert`、`python3 tools/verify_sources.py` 均通过；实际 rustc/cargo 为 1.97.1。Windows Actions 工作流已创建但未执行。不依赖 sibling 源码运行 workspace tests。D04 最新实现位于 engine/src/lib.rs、config/src/lib.rs；新增 5 个测试验证嵌套序列、同键冲突、vk_none、重复与跨阶段展开拒绝。

D03 最新检查：config/src/lib.rs 与 unique_json.rs；新增 4 个行为测试覆盖所有配置执行层的未知字段、JSON Pointer 转义、禁用规则保留、所有 profile 的基本类型、重复 JSON 成员与数值/Unicode 往返。上述 fmt/clippy/workspace tests 通过，总计 32 个。global/UI 设置暂未分类，明确拒绝；inactive profile 的完整功能字段在切换时重新编译，当前不声称已验证其可激活。

D04/D05 最新检查：引擎阶段 binding 树、SourceToken 区间预留、临时 suppression/已提交 pressed set 分离；增加 9 个引擎、3 个配置、1 个 CLI 测试，总计 45 个测试通过。时间为确定性纳秒，未执行任何硬件抓取。仅 alone 的 up 时检查已实现，held/delayed/simultaneous deadline 引擎仍待做。已知四个 basic 参数检查与保留，具体作用见 COMPATIBILITY。

D02/D04 设备记录修正：engine update_device 只保存每个 id 最高 generation/removed，4096 身份硬限额；旧移除幂等、旧 wheel/move 拒绝、新代际移除释放旧 owner。新增 5 个测试（含 10000 次同 id 重连、身份容量、Reset 保护），fmt/clippy/workspace tests 通过，总计 50 个。实际热插拔与完整 runtime 容量恢复尚未实现，见 ADR-0005。

D04 修饰匹配/消费：新增 modifiers.rs 与 modifier_lifecycle.rs；33 个上游标准 matcher 案例通过，1 个 Fn 案例准确跳过；10 个引擎、2 个 compiler、1 个 CLI 新测试，总计 63 个 workspace tests 通过。lazy 状态与 emitted intent 分开，ordinary up 不错误刷新；同来源提前释放和同键新来源正确区分。屏蔽快照独立预算及压力测试通过；fmt/clippy/fixture verifier 通过。新增 3 个固定源码 hash（当前 51 个），上游 matcher 原始复制未改。依然没有后端 submitted 状态、真机权限/故障验证，未完成完整 C1/C2。

D04/D05 repeat 动作：IR 升级为 Action { key, repeat }；最后 repeat:false 立即轻触，extra/up 宏始终完整轻触；阶段 rule 不提前结束，modifier consumed scope 仍等原 up。新增 4 个引擎、1 个 compiler 测试，总计 68 个通过；fmt/clippy/workspace tests 通过。没有 OS repeat 后端，物理 Repeat 仍不生成新 owner/重播宏。

D04 输出修饰作用域：Action.modifiers、lazy 叶标记与设备 neutral 刷新；十个 engine、三个 compiler、一个 CLI 新测试，总计 82 个 workspace tests 通过；fmt/clippy 通过。两个原始 HID oracle 比较 14/5 个边沿，standard branch 与 vk_none 顺序一致，明确未转换 Mach 时间；原文件/expected 不修改。source verifier 现有 54 个 hash。尚未实现显式 lazy/hold/halt、conditions、timers、submitted 状态或实际后端；完整目标保留。

后续更新必须记录：任务编号、实现文件、运行命令、目标 OS/build、结果、剩余限制。规划、mock/回放通过、编译通过、真机行为通过分别表述。

D04 设备条件/上下文：types/context.rs、engine/conditions.rs、engine/src/lib.rs 与 config 编译器新增四类 device 条件、规则/输出动作筛选、256 项有界库存、revision/有效期验证、三值 Unknown 与重连 metadata 失效。新增 9 个 engine、3 个 compiler、1 个 CLI 测试，总计 95 个通过；新增 device-context 回放覆盖过期后旧 binding up 与新 down passthrough。fmt/clippy/workspace tests 与 source verifier（现 55 个 hash）通过；原上游 expected 未改。Windows Hook 能力下设备条件准确拒绝，实际 Windows CI/设备 provider/捕获和系统恢复均未运行，Ubuntu 24.04.4 无输入硬件访问。ADR-0008 记录 Unknown 与上游默认值行为的兼容差异。本阶段没有应用、输入源或变量条件，完整目标仍未完成。

D04 应用/输入源条件：ContextSnapshot 增加 ApplicationContext/InputSourceContext；condition IR 新增应用与输入源 if/unless、ASCII 单行受限 Pattern，config 阶段预编译 regex。应用跨字段 OR、输入源字段 AND/列表 OR、缺失/非 ASCII/CRLF/过期 Unknown；配额 256 patterns/配置、1024 bytes/pattern、32 KiB 程序与 16 KiB DFA cache；属性 4096 bytes 上限。新增 4 个 engine、3 个 compiler、1 个 CLI 测试，总计 103 个通过；text-context 回放证明上下文变化时旧 binding release 和实时 up 动作条件。fmt/clippy/workspace tests 与 source verifier（56 个 hash；其中两个条件文件原已固定，新增 selector specifier）通过。regex 1.8.1 与 regex-syntax 0.7.1 从离线缓存固定加入 Cargo.lock，未宣称最新依赖版本。没有 OS provider、应用 ID 转換、窗口标题采集或真机延迟验收；Windows CI 未运行。详见 ADR-0009，变量/时间规则和后端/GUI/驱动仍未完成。

D05 typed variables：types/variables.rs、ActionTarget、Bound::Variable、规则条件和 config 编译器支持 i64/bool/String、variable_if/unless、set/unset/value/key_up_value。256 名字预留跨配置/旧 binding；Engine::new 改为 Result，激活失败不改现状；Reset 清值并回收旧名。设备移除完成已执行 action 的 key_up_value，不运行额外宏，此恢复偏差见 ADR-0010。批量动作先过滤，再依序执行，变量条件不依赖 OS context 新鲜度。新增 9 个 engine、4 个 compiler、1 个 CLI 测试，总计 117 个通过；variables 回放断言模式切换与持久值，默认输出不导出值。fmt/clippy/workspace tests、source verifier（60 个 hash）通过；Ubuntu 24.04.4 / Rust 1.97.1，Windows CI 和真机抓取未运行。浮点值、expression/key_up_expression、非键 modifier scope、simple 非键目标明确拒绝。时间规则、平台后端、session/GUI/驱动及发布仍未完成。
