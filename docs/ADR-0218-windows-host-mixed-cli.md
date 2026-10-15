# ADR-0218：Windows host/CLI 的显式 mixed 选择

run-windows/check-windows 增加 --capture-scope keyboard|staged-mixed，缺省keyboard；mixed必须显式staged-keyboard。Check沿完整PreparedConfig scope准入，输出offline/mode/scope/device readiness unverified，不创建worker/设备。

PreparedConfig.worker_arguments由冻结mode/scope派生私有argv；Windows host run_owned共用同prepared对象生成argv与seal原bytes，再沿原Supervisor四handle事务/Job/watchdog运行。CLI冷读显式scope，不能从配置是否有pointing_button猜测scope。旧run/stop/cleanup/control默认行为保持。

纯CLI/parser/config和PreparedConfig→private Arguments roundtrip在Linux测试；Windows host/native main尚未原生编译执行，不把Linux离线成功说成Hook/Job/鼠标接管成功。配置watch/GUI/driver与原生验收仍待，默认scope不拓宽。
