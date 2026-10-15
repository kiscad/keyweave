# ADR-0009：应用与输入源条件的受限正则

更新：ADR-0228已取代下述历史ASCII限定，现支持UTF-8文字/subject的字节正则，限额与Unknown边界保持。参考 [ADR-0228](../ADR-0228-utf8-byte-pattern-conditions.md)。以下保留最初设计和当时验证记录。

状态：已实现离线受限模式；2026-10-08。

引擎只接收 ContextSnapshot 中的应用/输入源属性；条件不会查询 OS。每个属性可独立 Unknown；整个快照过期时全部 Unknown。应用 bundle_identifiers/file_paths 两组 OR；输入源单个 specifier 字段 AND、列表 OR；unless 不反转 Unknown。输出动作条件与规则条件共用 IR。

Rust regex 与 C++ ECMAScript 不完全等价。第一阶段限定 ASCII pattern 和 ASCII 单行属性，长度分别 1024/4096 bytes；非 ASCII 或含 CR/LF 的实际属性是 Unknown。明确拒绝 lookaround、backreference、inline flags、Unicode/hex 转义与 Rust 集合运算、嵌套字符类/类内 shorthand、possessive 量词；只保留常用分组、alternation、量词、字符类、锚点和标准 ASCII shorthand/标点转义。应用标题不作为属性。后续 Unicode/原生应用 ID 转换需单独兼容 ADR，不能静默把 Windows exe 名等同于 macOS bundle ID。

正则在配置编译时构建，单个编译程序上限 32 KiB、DFA cache 16 KiB，当前选中配置最多 256 个 pattern（失败的外层 rule 也消耗编译额度，保守拒绝）。长度、语法或资源限额失败都诊断并禁用整条外层规则。引擎只匹配预编译对象；没有灾难性回溯。匹配成本有界但实际 Hook 性能仍需验证，未承诺真机延迟。

应用与输入源 provider 尚未实现，不因离线编译成功宣称 Windows/Linux 上下文可用。None/缺失属性保持 Unknown。空输入源 specifier 按上游为无约束，只有已知输入源对象时才 True。

ASCII shorthand 在类外显式转换为 ASCII 字符集合与 ASCII word boundary，避免 Unicode 自动机使常见 \w 配置耗尽编译限额；此转换只用于已验证的 ASCII 单行语义。32 KiB 限额可能拒绝合法但复杂的 pattern，诊断而非提高热路径资源风险。实例用 Arc 持有编译对象；没有 OS 查询、每事件编译或 Regex 回溯引擎。

验证：新增 4 个 engine、3 个 compiler、1 个 CLI 测试，workspace 总计 103 个通过；覆盖 search/anchor、缺失与 unless、Unicode/CRLF Unknown、input-source AND/OR、未支持语法、全配置 pattern 配额、上下文改变后旧 up 与当前动作筛选。context 超长应用/源属性原子拒绝也已覆盖。原始上游 oracle 未改，语义取自固定的 frontmost_application.hpp、input_source.hpp 与 BSL input_source_selector/specifier.hpp；记录 source hash 不宣称已执行 macOS 测试。

regex 1.8.1、regex-syntax 0.7.1 固定使用本机离线缓存版本（MIT/Apache-2.0）；后续升级需重新跑正则兼容测试与性能验证。实际 provider、平台 ID/输入法协议、会话隔离和故障恢复仍在后续任务。
