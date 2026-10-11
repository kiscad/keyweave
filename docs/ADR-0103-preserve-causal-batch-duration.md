# ADR-0103：保留被阶段队列推迟的批次持续时间

日期：2026-10-11。状态：纯阶段队列与终端actor的调度缺陷已修复；完整simple/complex deferred actor与原生duration入口仍未接入，不解除UnsupportedActionTiming。

## 问题与目标

上游 basic/event_sender.hpp:226 在一批to事件中累加hold_down_milliseconds，再生成释放与后续事件的时间。KeyWeave已有ActionPlan生成相应的时间跨度，但StageTimeline逐个将时间取max(事件时间,队尾)，会压缩整批延迟：前批队尾100，下一批down0/up50被变成down100/up100，丢掉50的跨度。这与计划中的duration语义矛盾，不能带着此缺陷接入完整deferred actor。

修复后该批成为down100/up150。不同stage仍独立推进，例如Simple队尾100、Output队尾20时，各自保留批内跨度，并保留等时间事件的原enqueue order。逻辑时间是最早执行时间，不承诺原生输出实际间隔或硬实时。

## 原子队列准入

enqueue先保留原容量、设备generation、order空间检查，再对本批每个stage计算一个平移量：该stage第一事件距max(原队尾,actor now)不足的部分。后续同stage事件用同一量checked_add并保持FIFO非递减；整个批次的effective时间预计算成功后，才移动事件/增加len/order。

任何时间相加溢出都返回TimelineError::TimeOverflow，RejectedBatch保留原始owned事件和原时间；队列、order、len均不变。不用saturating_add静默压到MAX，也不提前执行释放。容量仍为MAX_STAGE_APPEND1024/MAX_STAGE_EVENTS8192，临时时间数组仅与本批长度成比例，没有新增线程、IO或墙上时钟。

当队列已推进到100而新批原时间0/50，平移为100/150。已排队旧事件的earliest时间不被重写，late pop仍返回原earliest及实际delivered_at；本修复仅改变新批准入，不能从late delivery推断OS已经接受下发。

事件的原device/source/immutable snapshot保持；取消和device retirement照原有效时间/order移除未执行事件，只有actor已执行down的saved bound才能产生cleanup up。TerminalActor.enqueue在timeline准入成功之后才推进source范围/last_batch/slot预留，所以新增溢出错误保持其原子契约。原input_up仍通过同stage FIFO排在先前批次之后，不借PlanOrigin原时间提前释放。

## 验证与范围

Linux Rust1.97.1 locked/offline workspace/all-targets966项测试、Clippy -D warnings、fmt和固定来源60项校验通过。新增6项回归：多stage独立平移/跨度、actor已推进时late批次、整批overflow保原events/原queue、两terminal plans前批100/后批50输出c down100/up150、移除后批设备在125仅cleanup c且丢150未来up、actor overflow不改预留并允许同BatchId重试。既有hold/alone/delayed/lazy/变量/热配置/取消/终止与无输入harness保持通过。

纯Rust共享逻辑，无新unsafe/FFI/依赖；ABI未变不重跑oracle。未触发真实键盘/设备、原生duration配置、driver或安装。Windows此共享改动及前序native仍待编译验收，不将Linux/mock绿色当Windows证明。见 [验证记录](validation/linux-2026-10-11-causal-duration.json)。

下一步必须继续将按下/释放及生成动作穿过simple→complex→output的deferred actor，保存旧snapshot、source/slot绑定、held/delayed/halt/variable/取消语义，统合due预算、扩展预留和原生runtime输入回放。StageTimeline及TerminalActor只是此工作的一部分；输出阶段的通过不能代替完整规则或平台支持。simultaneous、Windows适配/验证、用户控制、图形消费者/hotplug/LED/UI/发布/driver等原完整目标保留。
