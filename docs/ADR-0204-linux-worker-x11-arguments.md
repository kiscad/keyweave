# ADR-0204：worker显式X11参数入口

SessionProvider.X11携helper、relative axes和可选pointing buttons，固定私有argv顺序 --x11-probe/--relative-axes/--pointing-buttons/--execution-mode staged-keyboard/input。数值集合为有界canonical递增decimal whitelist，buttons none可显式禁用；helper路径须absolute且不等uinput/input。传OsString给Command，不经shell，Debug隐藏路径；64 input上限保持。

LinuxArguments.capture_scope/output_spec从provider声明派生，worker在guard/endpoints身份核验后按同scope独立冷准备原sealed bytes，X11分支进入完整session_with_extension/cleanup结果分类。未知/deferred cleanup不报告Clean；deferred所有者丢弃后的Drop仍best effort，进程返回Cleanup且无伪造终端完成（parent-death保护仍在）。host侧暂拒X11参数，直至cold spec/helper认证接线完成，不让旧keyboard准备路径误接受新provider。

helper认证仍由可信host完成；private argv lexical检查不等同签名。原生X11入口只编译，无实际display/device运行；此阶段测试参数/封存选择和已有session边界，真实host/runtime/driver验收继续。
