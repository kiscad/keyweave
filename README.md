# KeyWeave

用 Rust 开发、面向 Linux 与 Windows 的键盘映射工具。名称取自“编织按键行为”，是本项目的工作名称，尚未核验商标或包名可用性。

当前交付是**源码评估与开发设计文档**，尚未实现映射程序、Cargo workspace、系统服务或驱动。原始 `../Karabiner-Elements/` 保持不变。

## 阅读顺序

1. [开发指导文档](docs/DEVELOPMENT_GUIDE.zh-CN.md)：可行性、范围、架构、平台方案、实现步骤、验收标准。
2. [源码分析与证据](docs/SOURCE_AUDIT.zh-CN.md)：本地版本、事件流程、关键源码、上游测试与分析边界。
3. [开发任务清单](docs/IMPLEMENTATION_BACKLOG.zh-CN.md)：带依赖和完成条件的 Coding Agent 工作单。
4. [AGENTS.md](AGENTS.md)：后续开发必须遵守的项目约束。
5. [项目状态](docs/STATUS.md)：区分规划、实现和实际验证。
6. [兼容配置示例](examples/karabiner-basic.json)：供后续 C2 行为验收，当前尚无程序可加载。

结论：可以重写规则引擎并移植主要能力。推荐先完成离线引擎和 Linux evdev/uinput 后端，再完成 Windows 用户态版本；Windows 驱动版本是独立工程与发布门槛。不能把 Windows 用户态版描述为 Karabiner 全功能等价版，也不能承诺所有桌面环境都支持应用条件和输入法切换。

分析日期：2026-10-08（Asia/Shanghai）。源码基线：`ec2fea8940f7254a748c052a890ee9db7113d9b1`，`version` 为 `16.3.11`。
