# ADR-0228：应用/输入源条件的 UTF-8 字节正则

问题：Pattern此前同时拒绝非ASCII配置pattern和subject，导致常见中文用户名/应用路径被判Unknown。上游fixed ec2fea8940f7254a748c052a890ee9db7113d9b1 的 vendor/vendor/include/pqrs/regex.hpp v1.3.0持有std::string与std::regex（默认ECMAScript），frontmost_application.hpp用string迭代器regex_search；不是wregex或Unicode scalar正则。

选择：使用已有锁定regex1.8.1的bytes::RegexBuilder，unicode(false)，在冷编译时把UTF-8 pattern非ASCII字节逐个写为内部\\xNN，保留原有语法/量词作用域。ASCII shorthand和word boundary仍按ASCII定义。UTF-8文本原样as_bytes参与search，不做NFC/NFD、大小写、宽字符或locale转换。例如^.$不匹配é，^..$匹配其两字节；é+量词仍只作用于最后字节，(?:é)+才重复完整序列。非ASCII字符类同样是UTF-8字节集合。此为上游字节行为迁移，**不是完整Unicode regex支持**。

保留1024bytes/pattern、256patterns/config、32KiB程序/16KiB DFA、配置时编译。CR/LF文本、缺失值、过期上下文仍Unknown，unless不能凭空True。lookaround/backreference/inline flags/用户Unicode或hex转义/集合运算/类内shorthand等继续明确拒绝；非ASCII转义也拒绝，避免错误的内部转写。内部hex只由程序生成，不开放额外配置语法。不引入C++生产引擎或依赖升级。

独立golden JSON供Rust测试与tools/verify_regex_oracle.py共用。后者在本机C++17 classic locale编译std::regex_search参考，检查UTF-8文字、Windows路径、点/量词/字符类、ASCII边界、NFC/NFD差异。Linux libstdc++通过不等于macOS libc++/不同locale原生验收；未运行上游macOS测试。Rust同时检查App/InputSource三态与完整JSON→冷准备。官方Rust实现文档来源为本地锁定regex1.8.1 src/re_builder.rs的bytes builder unicode/size_limit接口，无需网络依赖变更。

实际验证：1932工作区测试和30本机C++参考cases通过；格式、Clippy、源码62、既有C UAPI/X11 SDK及新regex oracle均通过，无初始失败。Windows新增测试调用API mock的真实UTF16解析/资源释放逻辑，并交付Pattern求值；未在Windows执行原生采样。用户JSON中的\uXXXX在JSON解码后是UTF-8文字，不等同被拒绝的正则\\uXXXX转义。
