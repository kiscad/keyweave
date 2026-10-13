# ADR-0158：组合键 IR 与有界 lookahead 协议

当前源码没有组合键缓冲实现。先补键盘 simultaneous/选项/阈值 IR 与独立有界 lookahead 决策，再将匹配组接到 Complex 多来源生命周期；本切片不把组合键当作首键普通映射执行。

证据来自固定上游 `basic/basic.hpp:260–410`：窗口包含恰好 threshold 的事件；同键跨设备以第一次 Down 为成员；成员在全部 Down 前 Up 则失败；detect_key_down_uninterruptedly=false 时其他 Down 打断未完成组，true 时可继续；down order 检查前缀；strict/inverse up order 只检查窗口内前缀，等至少 n-1 个 Up 或窗口成熟，不约束窗口外的后续 Up。key_up_when any/all 控制输出释放，options.to_after_key_up 在全体成员 Up 后一次触发。

协议保存完整原 Down/Up receipts（槽、来源、设备、旧 Arc 和原 earliest/delivered 时间），最多 32 个定义键/256 个缓冲事件。earliest 是本阶段窗口时钟，delivery 是执行时钟，二者不得互换。成熟窗口必须同时得到 `now>=end` 和上游已处理完 end 以内事件的 frontier（None 为明确 EOF，Some 为所有未观察输入 earliest 的下界）；不能先超时再丢掉已在上游 FIFO 的同窗口事件。时间反转、容量/来源错误拒绝时返回原 owned incoming receipt，旧缓冲不变。

参数输入沿既有严格整数/0..一天校验，effective simultaneous 阈值按上游 profile 参数 `adjust_values` 限为最多 1000ms（默认 50ms），raw JSON 不修改。本键盘子集拒绝重复定义键和超过 32 键；不能把上游更宽事件定义集宣称为已支持。成员只复制只读 observation metadata，原 DueEvent 继续唯一拥有在返回缓冲中，不新增其公开 Clone。

Matched 保存实际成员以及未消费的原缓冲顺序；ActiveChord 只根据保存成员的原 Up 更新 any/all/all-up 一次性事实。重复键/外部来源 Up 不冒充成员，设备/Reset 控制由后续拥有输出的 adapter 处理，协议不制造 native 输出或恢复证明。

ActiveChord.up 只借用原 receipt，调用方保留它用于实际绑定 Up 准入；接受或拒绝都不吞掉原来源凭据。匹配事实本身不证明捕获、输出接受或绑定回收。

配置完整解析本切片支持的键盘选项并保存原 JSON，仍报告 runtime 未接入、disabled，真实 host/worker 不准入；公共 Legacy/staged 构造器明确 UnsupportedSimultaneous，单键 selector 跳过。规则顺序/重叠候选仲裁、多成员共享输出/作用域/lease、超时正常规则回放和 native repeat 控制是接入的必要后续任务，不以纯协议测试代替。

## 后续接入必须证明的行为

- 在 Complex 阶段按规则顺序仲裁所有包含当前键的候选及普通映射，冻结第一次输入的旧 Arc、修饰视图和条件；失败后正常规则回放不能再次进入同一失败候选循环。重叠组合、reload 期间的新旧来源均须回归。
- 匹配组必须保留全部参与者的实际输入绑定和来源 lease；共享输出仍存活时不能提前退休任一祖先。any/all 的逻辑释放与各成员原 Up、实际输出/作用域完成分开，不能伪造真实 device 0 或普通原始 Up。
- 分别处理普通 after-up 与 group all-up 动作；动作条件使用正确的释放事件身份和 fresh context，合成快照的未知身份继续保留。组内 Held/Alone/Delayed、重复键以及后代投影都需要实际 actor 交付验证。
- 从拥有的 Simple 生成 FIFO、保存物理输入和预算状态计算真实 frontier；到期 lookahead 不能先于已排队的窗口内输入成熟，也不能被无限新输入拖延。沿既有每步有界 poll/原始 earliest/实际 delivery 规则调度。
- 为每个可能首成员预留组合展开预算，校验全部成员的 native 输入能力和所有输出分支；设备移除、Reset、失败恢复清组和未来任务，但不以物理 all-up 清掉 OS 接受账本。
- Windows 物理 Repeat 和 Linux Kernel repeat 的等待窗口停重复/恢复须分别实现和验证；协议事实不替代卸钩、REPORT、短/未知接受及独立监督的证明。全部 actor/replay/生命周期验收通过后再撤销 KW-SIM-001 和 UnsupportedSimultaneous gate。
