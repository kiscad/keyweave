# ADR-0200：X11 session 生命周期接线

新增 run_reporting_x11_session，ReportingStartup.claimed owners→guard/nonblocking progress预检→Stop-before-provider→冷X11Bootstrap→受控mixed startup→X11 worker→有限cleanup→确认后终端bridge。snapshot/spec仍由调用者冷编译，helper认证/独立监督/driver policy仍前提。默认provider/host CLI选择尚未改。

启动Stopped/Failed不构造Runtime report；有startup owner时先调度确认。StartupDeferred保原cause、scheduled cleanup和terminal writer；WorkerDeferred保active cleanup和terminal writer，resume继续同一任务，不重新boot/probe、不提前发送。启动完成只关闭未使用writer，运行完成才用确认后的cleanup report发送现有终端帧。clock读取失败传真实Result，保owner与原因。

此为组合已验证组件的原生接线，不新增OS FFI。完整native session只编译，现有控制启动、Runtime consumer停机、owned helper cleanup及真实terminal pipe测试继续；没有session/display/input/uinput运行验收。host argv/provider/cold config入口和硬件验收仍待，不能把类型检查当真实映射证明。

新增实际owned pipe的startup Stopped/Failed边界测试：未使用terminal仅关闭形成EOF，无Runtime帧，原失败cause保留；这不构成完整原生session运行证明。
