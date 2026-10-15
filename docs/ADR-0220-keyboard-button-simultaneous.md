# ADR-0220：键盘与按钮 simultaneous 及 mixed Alone 语义

上游固定ec2fea8的from_event_definition.hpp simultaneous逐项使用通用event_definition；basic.hpp:808按非零wheel取消alone，普通pointer movement不取消。本次拓宽现有有界simultaneous模型到keyboard+HID button1..32，不含consumer/其他事件；compiler仍逐项检查实际BackendCapabilities，默认Windows/Linux键盘caps不变。

Compiler保留首个原事件字段（key_code或pointing_button），不再强行转换首项为key_code。Chord定义、arbiter首receipt和Complex入口一致接受按钮；原32键/256事件/阈值/来源/旧Arc/顺序/容量语义沿用。Windows mixed冷校验遍历每个simultaneous成员，按钮6及以上即使不是首项也拒绝；不是仅校验Mapping.from。

测试实际JSON→mixed冷编→staged controller→Windows mixed账本，覆盖首项keyboard/首项button、组合成功与释放、不可用成员拒绝；另外验证Position不取消Alone、非零wheel取消、零wheel不取消。未注册原生Hook/注入，上游C++ tests未执行，不宣称完整simultaneous/macOS或Windows系统验收。
