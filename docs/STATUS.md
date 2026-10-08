# 项目状态

更新日期：2026-10-08。

| 项目 | 状态 | 证据 |
| --- | --- | --- |
| macOS 本地主程序与测试源码分析 | 已完成文档评估 | SOURCE_AUDIT 与 source-manifest.json |
| Windows/Linux 官方接口核验 | 已完成文档核验 | 开发指导文档中对应链接 |
| 新目录、开发指导、任务清单、AGENTS | 已创建 | 本目录 |
| 文档完整性检查 | 通过 | 59 个本地链接、48 个源码 SHA-256、JSON 语法、Markdown 围栏；上游工作区未变 |
| D00 来源固定与 ADR | 基础完成；companion 审计仍缺失 | tools/verify_sources.py、fixtures 原始复制、ADR-0001/0002 |
| D01 Rust workspace 与离线切片 | 已实现；Windows 编译待执行 | 4 crates、Cargo.lock、固定 1.97.1、32 个本机行为测试通过 |
| D02 HID 键名 | 部分实现 | 309 个标准键名/别名及 round-trip；native 转换/持久设备选择待做 |
| D03 配置编译与 CLI | 部分实现 | JSONC（含重复成员拒绝）、元数据往返、各执行层字段检查、整条禁用、validate/import/replay；高级 schema 待做；COMPATIBILITY 描述当前范围 |
| D04/C1、D05/C2 完整规则 | 部分实现，未完成 | 多动作跨阶段 down/up 生命周期、持键热更新、多来源 ownership 与展开限额；修饰、alone、timers 等选项仍准确拒绝；ADR-0003 |
| Linux / Windows 用户态后端 | 未开始 | 尚无程序 |
| Windows Rust driver 可行性原型 | 未验证 | D10 研究门槛 |
| macOS 上游测试执行 | 未执行 | 当前 Linux 环境，Apple framework 依赖 |
| 目标平台硬件、权限、故障恢复、性能 | 未执行 | 后续专用测试系统验证 |
| companion DriverKit 源码/许可证审计 | 未完成 | 本地子模块未初始化，固定版本远程读取失败 |
| 项目许可证、依赖/MSRV | 已选定 | MIT，保留 Unlicense/BSL 来源；Rust 1.97.1、MSRV 1.97、Cargo.lock |
| 公开名称可用性 | 尚未核验 | 发布前任务 |

下一任务：补齐 D03 结构/字段边界和 D04 规则生命周期，再实现 D05 时间规则。完整后端、session/GUI/驱动及发布目标保留，尚未达成全部开发目标。

本机验证：Ubuntu 24.04.4；`RUSTUP_TOOLCHAIN=stable cargo fmt --all --check`、`cargo clippy --workspace --all-targets --offline -- -D warnings`、`cargo test --workspace --offline`、basic/sequence replay `--assert`、`python3 tools/verify_sources.py` 均通过；实际 rustc/cargo 为 1.97.1。Windows Actions 工作流已创建但未执行。不依赖 sibling 源码运行 workspace tests。D04 最新实现位于 engine/src/lib.rs、config/src/lib.rs；新增 5 个测试验证嵌套序列、同键冲突、vk_none、重复与跨阶段展开拒绝。

D03 最新检查：config/src/lib.rs 与 unique_json.rs；新增 4 个行为测试覆盖所有配置执行层的未知字段、JSON Pointer 转义、禁用规则保留、所有 profile 的基本类型、重复 JSON 成员与数值/Unicode 往返。上述 fmt/clippy/workspace tests 通过，总计 32 个。global/UI 设置暂未分类，明确拒绝；inactive profile 的完整功能字段在切换时重新编译，当前不声称已验证其可激活。

后续更新必须记录：任务编号、实现文件、运行命令、目标 OS/build、结果、剩余限制。规划、mock/回放通过、编译通过、真机行为通过分别表述。
