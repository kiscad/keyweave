# ADR-0060：host 与 worker 共享无损配置编译与 Windows 输出准入

Supervisor 已接入原文件对象交付，但完整 host 还需要在创建 worker 之前拒绝有损配置与不可编码输出。新增 keyweave-worker 库中的 PreparedConfig：有界读取原始 bytes，复用 worker 的 Windows capabilities/generation1 无损编译，并执行纯 SessionActor/Engine 输出准入。此层不创建输入、输出 backend、进程、管道或 timer；不是输入会话/neutral/访问许可 token。

加载器拆成共享 byte reader 与借用 bytes compiler，PreparedConfig 保存同一不可变原始 byte image 和 Arc snapshot，不经 JSON 序列化重建。worker load_config 使用同一 compiler；UTF8错误保存 Utf8Error 的 offset 信息，不保存 FromUtf8Error 中整份原始 body，Debug 不应把无效配置内容写入日志。PreparedConfig Debug仅含byte length/generation，不显示profile名/body/规则。Windows seal将原bytes交给ADR-0057资源层，之后仍由Supervisor负责交付和所有权。

Windows runtime新增preflight_config纯接口：共享SessionActor.new使用的validate→Engine.new构造路径，涵盖所有action outputs/modifiers/设备condition与engine预算/未接入timing gate。不用fake backend或触碰Native API，不改变hot activate持键语义；不提前安装CaptureSession。父只做重复准入，真实worker仍在自己的install之前重验，不把父检查替代child检查。

新增startup preparation context guard，仅查看现有callback状态；host read/compile以及worker load_config在读取/编译之前拒绝keyboard callback，无FS/probe/进程API调用。Linux离线使用同样Windows编译/输出检查；guard在非Windows正常返回，不能声称已检查Windows桌面。参数builder使用同一严格九项/十一项私有协议规则：绝对OsString诊断路径、非零u32 marker与显式identity，固定六项extra参数，不混shell/任意开关。

验收应覆盖原JSONC bytes保留/parent-child同compiler、选中profile、disabled整规则/section拒绝、特殊native输出拒绝、engine duration/resource预算、UTF8错误redaction、8MiB/IO预算、argv roundtrip/marker宽度/非Unicode路径、callback零工作。Windows实测seal只有自有synthetic有效配置文件的create/read/close，不启动worker、不安装Hook/SendInput；不以纯preflight证明桌面/neutral/Hook活性或真实输出。

完整host还需要run/marker生成、可信exe定位、独立控制线程/用户stop、详细诊断与启动事务、热更新/正常capture验收。原Linux/D05/GUI/packaging/Rust driver/companion审计与完整目标继续。


实现与验收：Linux684、Windows x64/i686各652，alltargetsClippy/fmt/既有SDK/source60通过；两个181源hash匹配，所有父验证实际exit0。自有prepared文件test验证原bytes、compiler结果与最后close后的实际删除，无输入。[证据与失败记录](validation/windows-2026-10-10-prepared-config.json)。
