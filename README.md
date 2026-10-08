# KeyWeave

用 Rust 开发、面向 Linux 与 Windows 的键盘映射工具。名称取自“编织按键行为”，是本项目的工作名称，尚未核验商标或包名可用性。

当前已实现 Rust workspace、标准 HID 键名、JSONC 验证、基础映射、多动作序列、确定性离线回放、多来源输出所有权及持键热更新。系统后端、完整高级规则、GUI 和驱动仍在开发，进度见 STATUS。原始 `../Karabiner-Elements/` 保持不变。

## 构建与使用

工具链固定为 Rust 1.97.1，执行：

```sh
cargo test --locked --workspace
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/basic.json --assert
cargo run --locked -p keyweavectl -- replay --fixture tests/fixtures/sequence.json --assert
cargo run --locked -p keyweavectl -- doctor
```

目前 `validate` / `import-karabiner` / `replay` 都不访问真实键盘，不执行导入脚本；不支持的功能禁用整条规则并返回诊断。`import-karabiner` 只输出无损副本，发现不支持功能时写报告并拒绝生成可激活副本。`examples/karabiner-basic.json` 中的单独轻触功能尚未实现，因此此示例暂时会被准确拒绝。

本机安装的 `stable` 经核验实际为 1.97.1；离线开发环境用 `RUSTUP_TOOLCHAIN=stable cargo ... --offline` 复用已安装工具链，CI 仍使用固定 1.97.1。尚未运行 Windows CI，不将其配置存在视为已通过。

## 阅读顺序

1. [开发指导文档](docs/DEVELOPMENT_GUIDE.zh-CN.md)：可行性、范围、架构、平台方案、实现步骤、验收标准。
2. [源码分析与证据](docs/SOURCE_AUDIT.zh-CN.md)：本地版本、事件流程、关键源码、上游测试与分析边界。
3. [开发任务清单](docs/IMPLEMENTATION_BACKLOG.zh-CN.md)：带依赖和完成条件的 Coding Agent 工作单。
4. [AGENTS.md](AGENTS.md)：后续开发必须遵守的项目约束。
5. [项目状态](docs/STATUS.md)：区分规划、实现和实际验证。
6. [兼容配置示例](examples/karabiner-basic.json)：供后续 C2 行为验收；当前准确诊断未支持的部分。

结论：可以重写规则引擎并移植主要能力。推荐先完成离线引擎和 Linux evdev/uinput 后端，再完成 Windows 用户态版本；Windows 驱动版本是独立工程与发布门槛。不能把 Windows 用户态版描述为 Karabiner 全功能等价版，也不能承诺所有桌面环境都支持应用条件和输入法切换。

分析日期：2026-10-08（Asia/Shanghai）。源码基线：`ec2fea8940f7254a748c052a890ee9db7113d9b1`，`version` 为 `16.3.11`。
