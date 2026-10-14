# ADR-0179：显式 Linux relative Runtime 输入接线

将原生 relative packet 连接同一 CaptureDevice→FrameDecoder→Physical controller→PendingRelativeFrames→OutputPump。新增显式 CaptureDevice::new_with_relative 与 Runtime::new_staged_relative，核心模式仍 StagedKeyboard，relative-enabled 另作只读能力状态。默认构造、CaptureGroup/Console bootstrap 与 host/worker不选择新范围；文本消费者的键盘证明不能授权指针消费。

捕获冷预检要求完整相对 mask、支持轴、输出覆盖；物理抓取仍经过 owned output证据及双 neutral。relative scope 保留已有键码/property边界，支持现有键盘/consumer+相对轴的组合设备，普通 mouse按钮键码尚待接入。新 Runtime 只接收显式已准入/armed 的输入，由调用者证明独立 consumer-ready/监督。测试只用 mocks，不执行真实grab。

原始帧忽略既有无映射 metadata 后，依 key 顺序拆成 key 与 relative run；run保全部相对记录/顺序，输出 REPORT 边界按 stage 步骤重新建立，作为一次 Activity 注册。原 batch全部冷验证及总预算后才观察/入队；SYN_DROPPED仍提前停机。真实 registered pointer source注册原帧，forwarded receipt核验后入同一 mixed output步骤；键盘在pointer前，native prefixes/report/recovery沿既有pump。

缓存预算在controller准入前检查；控制/stop清相应缓存与尚未交付raw/logical，source不重用。设备移除先关闭物理捕获，然后实际controller控制与输出恢复；不为cancelled relative造输出。时钟、seq、64-work、context、emergency与输出错误停止策略保持。

本接线不是独立graphical consumer provider或hardware验收。后续继续普通mouse按钮/完整pointing设备准入、consumer授权、host/worker scope交付和原生验收，不以本库入口缩减完整目标。无新增unsafe/FFI/依赖。
