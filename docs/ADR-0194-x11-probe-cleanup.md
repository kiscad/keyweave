# ADR-0194：停机后 X11 probe cleanup 确认

新增 X11Cleanup 消费 X11WorkerReport，独占归还的 consumer owner；初始化请求 cancel，step 对同一 owner 做一次 poll_canceled。观察预算500ms/唤醒2ms，时钟倒退/失败/overflow/deadline 与 cancel/poll I/O errors各自保留，不替换 Runtime 的原 outcome。失败不销毁仍待退出的 owner，不创建新任务。

finish 仅在实际 Exited或无待回收job时完成，先 drop consumer（read-only output node/library等）再生成 X11CleanupReport；否则返回 boxed self 供同一任务继续观察。原 cancel_error、cleanup cancel/poll错误、timing failure 和确认类型都独立可见。到达预算后取消自动唤醒，但仍允许调用者显式 step观察同一 child退出；超时记录不被后来的exit抹除。

此接口在 Runtime 输入/输出 owner 已关闭后工作，不访问输入/uinput/display，不把kill请求/Drop当exit。终端bridge须用finish的确认结果，尚未自动接到host session入口。测试包含有界状态/owner保留及实际独占Python helper退出，非 native provider运行证明。
