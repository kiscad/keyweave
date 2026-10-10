# ADR-0068：Linux 启动前配置准备

Linux bootstrap 必须在创建输出与打开物理输入前拒绝不可完整执行的配置。增加 worker::linux_preparation::PreparedConfig：沿用共享 8MiB+1 有界读取、原始 JSONC/UTF8 bytes 保留与 generation=1 编译，但 capabilities 必须为 Linux；禁用规则/diagnostics 不能静默忽略。以调用者明确提供的 DeviceSpec 检验所有动作与修饰键，再以 Engine::new 检验执行语义；kernel repeat 是当前 Runtime 的唯一可执行来源，External 必须拒绝。保存通过校验的 spec 与 snapshot，避免准备结果和将来创建输出的能力脱节。

host check-linux 使用键表全部已知 native codes 的 KeyWeave/kernel-repeat/no-LED spec，仅作离线最大输出能力检查，明确不表示真实设备、consumer-ready、neutral、grab 或启动事务已完成。准备函数不打开设备、不创建 backend/process/timer，不启动引擎；后续 worker 在父保护/channel admission 后读取并重验同样准备，真实输出/输入准入仍独立。

为避免两个平台复制大小/UTF8/无损编译规则，将原 Windows 编译 helper 内部扩展为显式 capabilities 参数；原 Windows helper 与 PreparedConfig 公共行为不变。Linux 模块在所有目标可离线使用，Windows callback 外准备 guard 仍适用，不引入 native Linux FFI。

验收：原 bytes 与 generation、有限读取与 I/O/UTF8、无损拒绝、输出能力不足（包括修饰键）、External repeat 拒绝、跨平台 CLI。真实 Linux executable bootstrap、consumer readiness、hotplug/add、上下文与硬件验收继续。

最终验证：Linux736 tests及独立无设备harness，Windows x64/x86各710 tests、Clippy/fmt/既有SDK通过。[报告](validation/windows-2026-10-10-linux-preparation.json)。8个新增测试覆盖准备与用户CLI；未运行真实全局输入或设备抓取。
