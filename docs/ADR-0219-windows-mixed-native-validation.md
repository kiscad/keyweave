# ADR-0219：mixed 原生链接与 SDK oracle 补齐

源审计发现 ADR-0215 泛化 ProgressClient 后，Windows-only native_link 仍将SendInputBackend作为receive/publish的泛型参数，实际现在要求sealed SessionProgress，会导致原生编译失败；修正为CaptureSession<SendInputBackend>并同时加入MixedCaptureSession的receive/publish/receive_command实例。

生产链接检查增加MixedWorker install/step/run/stop、MixedCaptureSession install/poll/shutdown、HookSession mouse install/held/双close，以及SendMixedBackend/SendPointerBackend和mixed冷preflight函数地址。只black_box函数地址，不调用Hook或发送API；现有构造sender仍仅分配buffer。

Windows真实SDK C oracle增加MSLLHOOKSTRUCT完整layout/alignment、POINT、INPUT.mi与所有新鼠标flags/messages、XBUTTON、WHEEL_DELTA、WH_MOUSE_LL和injected flags。仍由既有x64/x86 MSVC /Zs检查，不生成或运行注入程序。不用Linux reprC单元测试替代SDK编译。

本轮不重试此前自动审批额度拒绝的原生操作；SDK/native link未执行、未证明。更新验证流程必须要求固定新源码快照分别跑两架构，不能沿用历史432/477等计数证明mixed源码。Linux检查只证明未影响已可执行测试；Windows编译修复结论来自类型源码审计，仍待原生确认。
