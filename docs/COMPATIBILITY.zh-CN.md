# 当前配置兼容范围

ADR-0135：GeneratedStageQueue/TerminalWork 接入同一 StageTimeline 的 Action/Pointer mixed FIFO，指针共用8192事件预算但无 producer/变量/动作 delivery slots。Simple→Complex 交付时取消非零 wheel alone；Output 两侧共同 prepare/commit，Terminal 实际同步 modifier 后返回精确 pointer receipt，交付前比 payload/source/device/time/Arc。PhysicalStep 排队 Capture，bounded poll 返回 forwarded_pointers（同一步 keys 先于 pointer）；删除 UnsupportedPointerScheduling。热更新保旧 Arc，device/Reset 丢未来 pointer 无假 Up。新增6/更新2 tests，Linux1247工作区用例/Clippy/fmt/source60通过，无初始失败，阶段未接线时临时unused warnings已消除。纯控制器完成mixed FIFO，原生pointer捕获/注入/接受、scope/global实际来源/simultaneous/native/Windows/硬件/GUI/包装/driver及原目标继续，native gate保留，无unsafe/FFI/deps。见 [ADR-0135](ADR-0135-mixed-pointer-stage-fifo.md)、[验证记录](validation/linux-2026-10-12-mixed-pointer-fifo.json)。

ADR-0134：新增 PhysicalStageController 私有拥有标准 InputEnvelope→单槽 Simple 根、active/pending 物理来源、单调 source cursor、device 墓碑与 context。原 Up 用旧 Arc/slot/source；Repeat 沿 Simple 保存 primary 和 Complex 当前 main/held 到实际可见已 emitted Terminal owner，只返 target、不造 Repeat intent/owner。seq/time/due cold 预检，Injected Pass，设备新代/Reset 清实际后代和来源；共享原 Engine bounded context 验证，poll 一步/fresh ctx。wheel/pointer 空队列同步 modifier/alone；future 排队动作前明确 UnsupportedPointerScheduling，混合 FIFO 尚待。新增 15 集成+1 source 耗尽 unit，Linux 1241 用例/Clippy/fmt/source60 通过，初轮 fixture/Clippy/编辑脚本失败已记录修正。scope/global actual source pool/指针队列/simultaneous/native/Windows/硬件/GUI/包装/driver 及原目标继续，native gate 保留，无 unsafe/FFI/deps。见 [ADR-0134](ADR-0134-physical-stage-controller.md)、[验证记录](validation/linux-2026-10-12-physical-stage-controller.json)。

ADR-0133：新增 SimpleStageActor 私有拥有 Simple 原计划/producer arena、Complex 生成与实际 actor、保存孩子 Up/完成计数。Down 一次选旧 rule，三方 guard 提交；原 Up 联合父 marker 与生成 Up，实际孩子全部完成再退队列/外部登记/父根。注册 Simple 未释放槽决定下游 EOF，空末尾 Up 显式 flush；poll 每次一有界步，下游 due 优先，reload 预检共享 Arc、device/Reset 实际回收先于生成/树清理，ID 不重用。新增 11 tests，Linux 1225 工作区用例/Clippy/fmt/source60 通过；fixture API/FIFO 时序及 Clippy 断言写法错误已修正并记录。Simple 高级分支/consumed scope 投影显式拒绝；仅已登记 Simple main actor，物理入口/repeat/全局来源/scope/context/simultaneous/native/Windows/硬件与原目标继续，native gate 保留，无 unsafe/FFI/deps。见 [ADR-0133](ADR-0133-owned-simple-main-actor.md)、[验证记录](validation/linux-2026-10-12-simple-main-actor.json)。

ADR-0132：ComplexStageActor 的 prepare_generated_child_input / prepare_generated_empty_input 将实际 Simple 父预备、Complex 生成和输入登记合为独占事务。guard 持有私有旧父凭据；两目的 current Arc 必须一致，普通旧 root 仍拒绝。abort/drop 不激活父、不产生变量/队列/输入/ID；空匹配只消费原 Down，无假 child。新增 6 tests，含热更新后的旧 Simple→Complex→Output Down/保存 Up/孩子先父回收、两侧容量拒绝、撤销及空消费。Linux 1214 用例/Clippy/fmt/source60 通过，初轮测试夹具 API 与变量 namespace 初始化错误已修正；无 unsafe/FFI/deps。仍非完整 Simple/allstage controller，scope/物理 EOF/全局来源/native/Windows/硬件及原目标继续。见 [ADR-0132](ADR-0132-retained-complex-input-transaction.md)、[验证记录](validation/linux-2026-10-12-retained-complex-input.json)。

ADR-0131：InputBatch区分unreleased（原Up已提交才减）与remaining（实际children完成），Complex private poll按同device仍有未释放input决定EOF sync，Reserved future slot不让lazy modifier提前flush；bare Terminal/Output public poll保原策略。最后empty/tap已结束input的原Up在无同device output recipes时显式sync并返intents，pending delayed不延长input EOF。新增5tests，Linux1208用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。仅Complex input group协议非真正物理Simple root EOF，完整controller/scope/source池/native/Windows/硬件及原目标继续；caller须收admit和poll两处intents。 见 [ADR-0131](ADR-0131-complex-input-eof-modifier-sync.md)、[验证记录](validation/linux-2026-10-12-complex-input-eof.json)。

ADR-0130：ComplexStageActor.prepare_input/PreparedComplexInput 独占root arena/input map/last ID预备，stage/current Arc/live device/ID/槽预算全部通过后commit才登记；abort返原owned plan/drop零ID/来源/metadata变更。register_input包新路径保语义，zero计划仅commit用ID无假root。可与上游PreparedScopedGeneration两侧先prepare，再commit，无业务拒绝（alloc/panic除外）。新增6tests，含8192源队列/16384目的预算/设备拒绝撤另一guard、空ID及真实Complex DueEvent→Output变量Down/Up完成回收；Linux1203用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。仍为root协议，旧Simple父新child/完整Simple owner/scope/source池/物理EOF/native/Windows/硬件及原目标继续。 见 [ADR-0130](ADR-0130-prepared-complex-input.md)、[验证记录](validation/linux-2026-10-12-prepared-complex-input.json)。

ADR-0129：修复Complex input empty计划绕过事件级device检查：register_input在任何ID/source/input变更前，检查Generated与Terminal两侧精确已观察且未removed代际。unknown/removed/stale/future未观察均拒绝，empty与nonempty统一；reject不latch Failed、不占ID/预算，同ID可在合法新代观察后使用，Reset保tombstone。新增3tests，Linux1197用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。仅内部history非真实FD/HID/lease存活证明，低级纯选择/空计划语义保留；全stage/controller/scope/source池/native/Windows/硬件及原目标继续。 见 [ADR-0129](ADR-0129-complex-input-device-admission.md)、[验证记录](validation/linux-2026-10-12-complex-device-admission.json)。

ADR-0128：ComplexStageActor接入delayed并撤剩余cold gate：pending任务保原Up后Live父；新Down先take旧task并生成Canceled，再选择新input，超时Invoked，旧Down snapshot+fresh触发view/context只处理一次。next_deadline合并三者，due output先drain、timer最早优先/同刻held先；finish等delayed和所有actual child，device/reset丢task不执行退出动作。新增9tests（Up后保留/取消影响新选择/ctx与旧rule/held同刻halt/empty最大ID/控制丢任务/overflow/snapshot预算/变量与wheel），Linux1194用例/Clippy/fmt/source60通过，无测试失败，初轮冗余wildcard warning已清理，无unsafe/FFI/deps。Complex基本分支组合已接，不是全项目完成；Simple/scope/物理EOF/source池/context/simultaneous/native duration/Windows补验/硬件/GUI/包装/driver继续。 见 [ADR-0128](ADR-0128-complex-delayed.md)、[验证记录](validation/linux-2026-10-12-complex-delayed.json)。

ADR-0127：ComplexStageActor接入held：checked实际Down+旧threshold deadline纳入next_deadline，任意新Down（含变量）/目标Up/设备/Reset撤销task。poll_with_context使用当前context，旧poll为Unknown；due输出先交付，再timer生成旧main保存Up/逻辑scope/变量Up，fresh filter held非tap计划，替换main关联但旧child保至实际完成。原Up释放新held并可继续alone/afterUp。新增9tests，Linux1185用例/Clippy/fmt/source60通过；首轮旧held-gate断言已更新delayed并记录，无unsafe/FFI/deps。失败可在旧Up提交后latch Failed，非全事件原子/OS恢复证明；delayed仍拒绝，Simple/scope/物理EOF/source池/context/native duration/Windows补验/硬件及原目标继续。 见 [ADR-0127](ADR-0127-complex-held.md)、[验证记录](validation/linux-2026-10-12-complex-held.json)。

ADR-0126：ComplexStageActor接入alone资格与分支：新key/button/consumer Down取消旧候选，变量不取消，Up不恢复资格；elapsed严格小于旧timeout，非零wheel两轴取消，零wheel不取消。main Up后先用旧Down modifier snapshot/fresh Up-time条件生成alone tap，再afterUp读其生成view；父等所有children结束，snapshot extra入同arena预算。新增7tests（边界/跨设备按钮/wheel/变量/原modifier已Up后snapshot重建/alone影响afterUp及halt/旧rule与Reset），Linux1176用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。wheel入口仅资格与clock非pointer输出；held/delayed仍拒绝，Simple/scope/物理EOF/source池/context/native duration/Windows补验/硬件及原完整目标继续。 见 [ADR-0126](ADR-0126-complex-alone.md)、[验证记录](validation/linux-2026-10-12-complex-alone.json)。

ADR-0125：ComplexStageActor接入after_key_up：保存main Up/逻辑scope/变量Up先提交，旧rule在Up生成时间读当前目的view/context一次过滤tap数组，FIFO接旧main Up/marker后；非空另建同父child，空filtered/haltskip不占batchID，所有孩子实际完成后才finish。main/branch halt在生成提交时记录，incoming halt仍属上游；任一child完成只清自身main关联。新增8tests，Linux1169用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。branch准入失败可发生在main Up已提交后，latch Failed、停止普通poll/input并要求stop/reset恢复，不冒充全事件原子。held/alone/delayed仍明确拒绝，Simple/scope/物理EOF/source池/context/native duration/Windows补验/硬件及完整目标继续。 见 [ADR-0125](ADR-0125-complex-after-key-up.md)、[验证记录](validation/linux-2026-10-12-complex-after-up.json)。

ADR-0124：新增 ComplexStageActor 私有拥有输入原计划/producer arena/main child关联/Output actor与单调IDs，实际admit Down/原Up、poll输出完成后拆child登记并finish父/retire根，设备/reset先清Output再清producer并返回上游须丢弃batch IDs。due要求先poll，DeadlinePending尚未选择；其他admit错latch Failed，不重选已失败Down。保存原Up recipe允许modifier Down.lazy=false/Up.lazy=true，Terminal释放采用planned Up.lazy；exact私有output batch receipt关联halt。新增12tests，Linux1161用例/Clippy/fmt/source60通过；初轮1项EOF flush断言已修正并记录，无unsafe/FFI/deps。当前main阶段组件冷拒绝Complex高级分支，不改native runtime/duration gate；Simple/跨stage scope与物理EOF/全stage来源池/分支/context调度/Windows补验/硬件及完整目标继续。 见 [ADR-0124](ADR-0124-owned-complex-main-actor.md)、[验证记录](validation/linux-2026-10-12-complex-main.json)。

ADR-0123：ProducerDecision.prepare_up/PreparedProducerUp 精确校验原Up的slot/stage/device/source/旧Arc/target（含变量operation）/lazy/halt及因果时间，commit一次记录up_delivered_at，abort/drop可重试。BindingArena<ProducerDecision>.prepare_producer_up只接Live状态，无任意live_mut；finish_producer须原Up已提交并等子登记清空，generic take/cancel仍供控制清理。新增8tests含同刻迟交付/错配/重放、旧配置保存Output Up、8192满队列拒绝后父标记重试及empty原Up无假输出；Linux1149用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。caller必须在下游Up准入成功后commit父标记，再安排分支并等实际scope/孩子清理；完整controller/scope/source池/分支/native duration/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0123](ADR-0123-producer-original-up-barrier.md)、[验证记录](validation/linux-2026-10-11-producer-up.json)。

ADR-0122：BindingArena.prepare_consumed_down/PreparedConsumedDown 为matched empty main只预备原Reserved父激活，无子batch/边/槽/extra。Complex/Output empty child事务消费完整guard，events/releases须皆空；commit共同激活父及提交空目的计划，abort/拒绝零状态。Output保strict ID/原容量规则，空commit只用ID无recipe/queue/owner；Complex给no-op Up凭据。新增9tests（旧Arc empty/abort/drop/非空拒绝/receipt与origin/满16384 arena及4096 Output拒绝重试/保存旧rule后AfterUp duration孩子先父），Linux1141用例/Clippy/fmt/source60通过，无初始失败，无unsafe/FFI/deps。只完成empty消费绑定原语，caller须配对frozen decision并管理原Up；完整controller/Up屏障/scope/source池/context/分支/native duration/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0122](ADR-0122-consumed-empty-producer-down.md)、[验证记录](validation/linux-2026-10-11-empty-producer.json)。

ADR-0121：新增 ProducerDecision 保存输入事件/旧Arc/实际delivered_at/目的stage与一次selected main indices。GeneratedStageQueue/OutputStageActor.decide_producer_down 只读采用原事件快照和目的生成modifier/variable view，context按交付时间；普通selector仍读当前配置。plan_main重试保冻结main/原source/device/time，空eligible匹配consume不fallback；pass-through保target/lazy/保存原Up，incoming halt仅上游元数据，分支仍fresh filter。新增12tests，含热更新/变量/TTL/生成modifier视图、变量pass/空main及实际非Clone producer联合Output准入原Up，Linux1132用例/Clippy/fmt/source60通过，初始无失败，无unsafe/FFI/deps。Decision非激活/授权；controller须仅选择一次并保存，普通old root仍拒绝，完整controller/empty parent/Up屏障/scope/source池/native duration/分支/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0121](ADR-0121-frozen-producer-down-decisions.md)、[验证记录](validation/linux-2026-10-11-producer-decisions.json)。

ADR-0120：BindingArena.prepare_child_with_down 校验已交付 Down 的原slot/stage/device/source/旧Arc与因果时间，仅 Reserved 父可在 Reservation 中待激活。commit才将父变Live并登记孩子，drop/下游准入失败无父半提交；普通prepare仍需Live父。兼容 Complex 生成二方及 Output 三方guard，激活不重复收父槽预算；child start不早于delivered_at。新增11tests，含实际Complex queue迟交付receipt→Output联合激活、孩子先父原Up清理、8192/16384/512边界拒绝和receipt/重复激活校验；Linux1120用例/Clippy/fmt/source60通过。首轮测试构造器编译错误已修正并记录，无unsafe/FFI/deps。DueEvent是caller一致性证据非认证，payload不得预先执行副作用；更早生成/pop不回滚，完整controller frozen retry/fault、scope/source池/native duration/分支/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0120](ADR-0120-atomic-producer-down-child-admission.md)、[验证记录](validation/linux-2026-10-11-producer-down-child.json)。

ADR-0119：GeneratedStageQueue.prepare_complex_child_scoped 消费 Live Simple 父的 child Reservation，仅供 Complex 目的队列，联合登记、生成环境/交付预算/时间线并自动计 snapshot extra。PreparedGeneratedChild commit 才创建边和原Up凭据，abort/拒绝零状态；旧父仅授权 current/older exact Arc，普通旧root/future/same-gen foreign/错配/parentless/Reserved父仍拒绝，Output仍用三方事务。新增10tests（含手动旧Simple→Complex→Output组合、孩子先清理、8192满队列、16384预算、变量Reset/设备清理），Linux1109用例/Clippy/fmt/source60通过。首轮3项测试预期错误及第二轮2项测试时钟倒退已修正并记录，未改生产时序。实际producer/controller、跨stage scope/global source ID池/native duration/分支/simultaneous/Windows补验/硬件及原目标仍待。 见 [ADR-0119](ADR-0119-retained-simple-complex-generation.md)、[验证记录](validation/linux-2026-10-11-complex-child-generation.json)。

ADR-0118：BindingArena 将 Down 身份槽与 extra_sources 合计限制为16384；Reservation.with_extra_sources 只预备，commit 才计入，abort/拒绝无预留或边残留，后续所有 slot 准入计既有 extra。Output snapshot child 自动在三方事务预留 scope 来源；单槽 Up、输出完成或设备清理不提前释放外部登记预算，整批 retire/cancel/subtree 清理才回收并返回 extra 数量（非真实 owner/OS 恢复证明）。新增9tests，Linux1099工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps。仅同一 arena 的预算，完整 Simple/Complex producer、跨stage scope、全局 source ID 池/native duration/Windows补验/硬件及原目标继续。 见 [ADR-0118](ADR-0118-binding-tree-extra-source-budget.md)、[验证记录](validation/linux-2026-10-11-binding-source-budget.json)。

ADR-0117：OutputStageActor.prepare_child/prepare_child_scoped 消费上游child Reservation，三方guard同时持有arena预留和生成/实际执行准备，commit才登记父子边及输出；abort/任一拒绝均无变量/作用域/队列/来源/ID/边副作用。crate-private ChildAdmission精确batch/stage/start/device/source/旧Arc/Down槽数，须实际Live Complex父及下一阶段；只允许exact current Arc或更旧generation，普通旧root/parentless/future/same-gen foreign Arc仍拒绝。旧schema/缓存变量namespace验证保留，recipe只读parent关联；Output完成后外部登记仍需controller清理，Reserved登记不伪造Live或Up。新增14tests，Linux1090工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps。完整Simple/Complex producer、跨stage scope传播、统一source预算/context/分支/native duration/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0117](ADR-0117-retained-parent-output-child-admission.md)、[验证记录](validation/linux-2026-10-11-output-child-admission.json)。

ADR-0116：TerminalActor/OutputStageActor.physical_repeat_target(BatchId) 从第一saved-Up主slot的实际Live Bound查询重复目标，共享原Engine保留叶helper，必须keyboard非modifier、emitted、精确source owner且未hidden。Reserved/Released/tap/repeat=false/变量/consumer/button/modifier/Up已准入/取消/设备代际/Reset/Failed返回None；不重选配置、不重放macro、不创建owner/intent/clock，附加modifier不替代主slot。BindingArena.live只读无Clone。新增8组合+1arena+1私有source/visible/emitted注入tests，Linux1076工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps。仅目标查询，backend仍需desired/submitted证据与唯一repeat来源；LinuxKernel策略不变，native runtime和完整Simple/Complex传播尚未接，duration gate/旧父新孩子/scope/source预算/分支/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0116](ADR-0116-output-stage-physical-repeat.md)、[验证记录](validation/linux-2026-10-11-output-repeat.json)。

ADR-0115：新增 OutputStageActor 私有拥有 GeneratedStageQueue/TerminalActor/每批recipe，统一初始与原Up双预备准入、匹配生成视图、实际交付核对及完成后自动退休。Terminal crate-private receipt保完整action/slot/device/source/旧Arc/earliest；scope marker只推进生成clock不取其他action。raw中性scope、empty推进原ID但无recipe；control/cancel/reset两侧协调、reload双check后commit同Arc，实际绑定/scope结束才删除recipe与交付预算。失配/执行错误latchFailed，普通操作停止至Reset，错误不证明输出回滚/OS接受。新增12集成+4私有注入tests，Linux1066工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps。仍仅Output目的stage；完整Simple/Complex actor、跨stage scope/旧父新孩子/source统一预算/context/分支/halt调度/native duration/simultaneous/Windows补验/硬件/UI/包装及原目标继续。 见 [ADR-0115](ADR-0115-owned-output-stage-actor.md)、[验证记录](validation/linux-2026-10-11-owned-output-stage.json)。

ADR-0114：GeneratedStageQueue.prepare_input_up_scoped/TerminalActor.prepare_input_up 提供保存原Up的独占预备事务，None保原no-op、错误保原类型；两侧全部通过后commit才改scope/变量Up/交付账本/一次标记/队列/时钟。StageTimeline.prepare_enqueue_at先验时钟，commit追加并advance，abort零状态变更，旧earliest不重写。scope-only私有ScopeUp、due先drain、oldArc/原duration语义保留；普通input_up包装新路径。新增8tests含两侧8192满队列拒绝/原recipe重试、DeadlinePending、abort200后150合法、旧变量Arc和仅scope marker。Linux1050工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps。调用方仍需同批次/两侧None一致检查，完整stage actor/交付退休协调/scope传播/旧父新孩子/source预算/native duration/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0114](ADR-0114-prepared-original-up.md)、[验证记录](validation/linux-2026-10-11-prepared-original-up.json)。

ADR-0113：TerminalActor.prepare/prepare_scoped 接入独占 PreparedTerminalAppend，组合 timeline guard、BindingArena reservation、待插入Batch/source cursor/lastBatch/scope预算。丢弃零状态变更，commit无业务拒绝，实际scope仍到poll才打开；原enqueue包装保Output/currentArc/严格ID及容量语义。生成/执行两侧可全部prepare后commit，任一拒绝不提交另一侧；新增8集成+1私有来源耗尽测试，含8192满队列同ID重试、变量生成/交付差异、实际scope DownUp与显式退休、pending取消及空plan原ID规则。Linux1042工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps；分配失败/panic不在原子保证内，消费plan的abort不退原plan。saved-Up联合事务、完整stage actor/交付退休协调/scope传播/旧父新孩子/source统一预算/native duration/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0113](ADR-0113-prepared-terminal-admission.md)、[验证记录](validation/linux-2026-10-11-prepared-terminal-admission.json)。

ADR-0112：StageTimeline 与 GeneratedStageQueue 提供独占借用的 prepare/commit/abort 准入。prepare 完成原校验与 FIFO 固定位移，丢弃 guard 不改事件、变量、修饰视图、交付预留或持久 scope；原 enqueue 包装 prepare+commit。scoped 凭据仅在 commit 交付，原计划可同时用于另一队列准备；commit 无业务拒绝，分配失败/panic 不含在原子保证内。新增8个两队列拒绝/成功、无 Clone payload、变量和scope abort/同ID重试测试；Linux1033工作区用例、Clippy/fmt/source60通过，无unsafe/FFI/依赖。TerminalActor预备事务、saved-Up联合准入、完整stage actor/scope传播/旧父新孩子/source预算/native duration/simultaneous/Windows补验/硬件/UI与原目标仍待。 见 [ADR-0112](ADR-0112-prepared-stage-admission.md)、[验证记录](validation/linux-2026-10-11-prepared-stage-admission.json)。

ADR-0111：GeneratedStageQueue新增私有DeliveryLedger，修复future tap逻辑Up已生成但实际待交付时槽位可重用。初始Down新批次/同组source/device/旧Arc，活动批次只接保存Up；overlay+交付预留原子随时间线提交，失败无Ghost。每槽Down交付/Up生成/Up交付进度及snapshot额外来源预留保至显式retire，须全部交付且scope原Up完成；caller另验实际子绑定/scope，非OS接受证明。cancel/device/reset同步回收，4096批次/16384slots+snapshot每队列有界，未统一全输入source pool。Linux1025工作区用例/Clippy/fmt/source60，新增8tests，旧16k及8192fixtures分组保精确边界，无unsafe/FFI/deps。完整stage actor/scope传播/旧父新孩子/source预算/context/分支/native duration gate/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0111](ADR-0111-generated-delivery-slot-reservations.md)、[验证记录](validation/linux-2026-10-11-delivery-reservations.json)。

ADR-0110：BindingArena 每槽child BTreeSet邻接保原严格stage/source/device/旧Arc；visit_subtree只读给出原metadata和Reserved/Live/Released，根须Live、无T Clone。cancel_subtree先完整postorder预检再独占拆树，孩子先父/反槽及反child ID，owned记录保旧origin/parent/真实Live payload；未执行空子批次亦返回用于丢未来队列，单孩子拆边不影响兄弟。TerminalActor取消已接新recipe及原Arc，当前只有Output根故根队列丢弃不变量保留。4096batches/16384slots/深度3边界，非硬RSS/time/native恢复证明。Linux1017工作区用例/Clippy/fmt/source60，新增6tests含4096最大树，无unsafe/FFI/deps。scope差分传播/完整stage actor/普通键变量孩子/预留继承/来源预算/context/分支/native duration gate/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0110](ADR-0110-saved-binding-tree-inspection-and-cancel.md)、[验证记录](validation/linux-2026-10-11-binding-trees.json)。

ADR-0109：GeneratedStageQueue.enqueue_scoped 将frozen scope接入本目的stage生成匹配视图；main/held最后modifier consumed引用保至原Up生成，非modifier/snapshot tap生成后恢复。计数隐藏支持交错，stale captured owner槽/活动scoped batch拒绝重绑定；最多4096原Up recipes、16384引用及含snapshot临时owner峰值准入有界，队列失败零逻辑/变量/时间变更。ScopedGeneration不可Clone/私有queue Arc+原recipe，一次Up可跨activate旧Arc，满队列保状态重试，取消/device使Lifecycle、reset使Provenance；不是native身份或恢复证明。Linux1011工作区用例/Clippy/fmt/source60，新增10tests，无unsafe/FFI/deps。尚未向下一stage传播作用域差分，完整stage actor/旧父新孩子/来源预算/context/分支/native duration gate/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0109](ADR-0109-generated-modifier-scope-visibility.md)、[验证记录](validation/linux-2026-10-11-generated-scopes.json)。

ADR-0108：GeneratedStageQueue 按目的stage组合独立生成时modifier/变量Environment+StageTimeline；对应Simple→Complex/Complex→Output selector读目标逻辑视图，enqueue就更新lazy/variable DownUp，poll不重放。overlay逐事件校验slot/旧Arc/device/source/target、16384 live峰值，队列成功后commit，失败原events/变量/modifiers/队列不变；当前Arc新Down、旧保存Up可跨activate，历史变量有界并集至reset。控制返回logical slots/discarded events、保存变量Up反向清理，但这些不是实际owner完整清单。Engine共享strict/missing0/unless variable_truth。Linux1001工作区用例/Clippy/fmt/source60通过，新增7组合测试及16k边界，无unsafe/FFI/deps。生成作用域/context、完整stage执行actor/父子/来源预算/native duration gate/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0108](ADR-0108-generated-stage-environment.md)、[验证记录](validation/linux-2026-10-11-generated-stage.json)。

ADR-0107：ScopedActionPlan 同次 frozen selection 生成 plan+私有scope，不可换配；TerminalActor.enqueue_scoped 首执行开scope，快照独立owner/source预留+consumed/额外modifier计数隐藏，交错结束只解除自身引用。main/held最后modifier等原Up，repeat=false无savedUp亦有私有ScopeUp；早Up保持FIFO duration，非modifier初始数组结束恢复逻辑/保原lazy同步。取消/设备/reset清理仅实际payload，exact16384隐藏引用失败零owner变更并latch普通执行至reset；snapshot额外来源/总16384owners/512keys准入有界，时间线失败不改预留/来源/ID。Linux994工作区用例/Clippy/fmt/source60通过，新增12tests，无unsafe/FFI/deps。仅Output-stage/currentArc准入，完整Simple/Complex stage视图及旧绑定子批次准入、分支/来源预算/native duration gate/simultaneous/Windows补验/硬件与原目标继续。 见 [ADR-0107](ADR-0107-deferred-modifier-scopes.md)、[验证记录](validation/linux-2026-10-11-modifier-scopes.json)。

ADR-0106：RuleSelection.select_branch 新增 Held/Alone/AfterKeyUp/DelayedInvoked/DelayedCanceled 的生成时 fresh 条件过滤与拥有旧 Arc 的 BranchSelection；Unknown拒绝、halted不求值、缺失delayed区别empty。plan复用原planner，held最后repeat原Up，其余tap_all，旧device/source/stage/options保持；modifier policy仅元数据，完整作用域/分支准入仍须actor。Engine共享filter_actions，held旧孩子清理后一次filtered数组用于lastmodifier和执行，消除重复条件求值；变量activate保历史有界并集至reset已确认无丢名。Linux982工作区用例/Clippy/fmt/source60通过，新增6tests含真实Engine held变量全数组过滤/实际modifier DownUp；无unsafe/FFI/deps。完整延迟stage actor/native duration gate、source预算/simultaneous/Windows补验/硬件及原目标继续。 见 [ADR-0106](ADR-0106-frozen-extra-branch-selection.md)、[验证记录](validation/linux-2026-10-11-branch-selection.json)。

ADR-0105：共享 BindingArena 保存 Reserved/Live/Released 槽，prepare 独占 guard 在时间线成功后 commit，失败丢弃不改预留；父 Live 槽仅允许同旧 Arc/设备/source 的下一阶段子批次，孩子未退休时父 Up/取消拒绝。cancel 仅倒序返回真实 Live payload，retire 要求全部已释放；4096 批次/16384 槽有界。TerminalActor 已接实际 Bound/down/up/取消/reset，来源分配仍原 actor，空计划无幽灵批次。新增3测试，Linux976工作区用例/Clippy/fmt/source60通过，无unsafe/FFI/deps。完整 Simple/Complex 延迟 actor、额外分支、变量旧命名空间和来源预算仍待接，native duration gate 保留；Windows原生补验、硬件与全部原目标未完成。 见 [ADR-0105](ADR-0105-saved-stage-binding-lifecycle.md)、[验证记录](validation/linux-2026-10-11-stage-bindings.json)。

Engine已共享不可变阶段规则选择，first-match/一次主动作过滤/实际modifier侧别/旧snapshot保留；另提供Simple→Complex→Output主动作计划接口，973测试通过。该接口仍需deferred actor的owner/slot/up/extra分支接入，原生duration gate保留，Windowsnative待验。见 [ADR-0104](ADR-0104-immutable-rule-selection.md)。

阶段队列已修复受阻批次的持续时间压缩，整体平移保原跨度，overflow原子拒绝；966测试通过。该组件及TerminalActor不代表完整duration规则入口，Engine仍拒绝，simple/complex deferred actor/native接入和Windows补验继续待做。见 [ADR-0103](ADR-0103-preserve-causal-batch-duration.md)。

Linux --watch-config 已接后台父目录通知、选定文件名过滤、200ms quiet/持续1s调度上限，原周期兜底保留；通知失效缓存错误并回退，不认证正文或推进代号。960测试与60s周期/5s内自动响应rename/写入/恢复/删除的无输入harness通过。parent替换/外部symlink目标可能依赖轮询；无自动directory重新注册，实时通知/外部操作、成功console硬件及Windows适配仍待。见 [ADR-0102](ADR-0102-linux-directory-notice-debounce.md)。

Linux run-linux 已接显式 --watch-config 和100～60000ms间隔/default1000，console专用，None父侧早拒绝；baseline使用同一次冷准备正文/密封，终态汇总/100ms原任务回收。953测试与真实有效配置None无设备链路通过；成功console热更新未硬件验收，当前仍正文轮询，无目录通知/去抖/实时提示或外部reload命令，Windows待实现。见 [ADR-0101](ADR-0101-linux-watch-command.md)。

Linux host 库已提供固定间隔后台正文监听/手动读取调度，确认前不更新 active 基线，拒绝/取消保旧配置，Stop 沿用原监督并关闭 source；948测试及第三 actual-main 无输入 watcher 场景通过。用户 run-linux 尚未接入监听器，Windows 未提供此组件，真实 console/输入验收仍待做。见 [ADR-0100](ADR-0100-linux-config-watch-scheduler.md)。

Linux提供单任务后台regular配置文件读取，长度1..8MiB、元数据变化拒绝、Stop丢弃晚到正文，938项测试通过。raw snapshot不等于schema/激活或原子认证；watcher调度/host集成/用户reload仍未提供，Windows文件源未实现。见 [ADR-0099](ADR-0099-bounded-config-file-source.md)。

真实父子进程无输入harness已验证NativeSession Active准入、96KiB正文分段、实际后台编译/纯Engine激活、Accepted/Rejected/lostack与原progress/Stop/实际Exit；931libtests+两场景通过。合成Active epoch/session不证明硬件消费者，terminal仍Missing/recoveryfalse，watcher/用户reload与真实console验收待做。见 [ADR-0098](ADR-0098-native-no-input-reload-harness.md)。

Linux NativeSession提供单在途raw reload队列/有效确认/observed代号API，原pump同时监督并按Stop/故障关闭通道；931项测试通过。真实Active admission/console热更新与硬件未验，watcher和用户reload命令仍未提供。见 [ADR-0097](ADR-0097-linux-host-reload-transactions.md)。

Linux真实worker main已按独立FD模式接reload Controller与guarded console bootstrap/活动loop/terminal收尾，host console默认选该spawn；None永久无设备/78。923项测试通过，成功console新模式未硬件实测。parent事务管理、watcher和用户reload命令仍未提供。见 [ADR-0096](ADR-0096-linux-reload-worker-entry.md)。

Linux console活动循环提供WorkerExtension入口与reload Controller adapter，新增独立2s父请求活动预算、cached console TTL、5ms推进与capture-first停止/故障恢复。920项测试通过；真实main/host仍使用旧模式，成功console扩展与硬件未验，watcher/reload命令仍待连接。见 [ADR-0095](ADR-0095-linux-worker-control-extension.md)。

组合 reload Controller 已实现独立前后证据检查、固定事务超时、后台准备、实际激活、部分确认和原端点复用；916项测试通过。实际原生worker/host循环与用户watcher/reload命令仍未接入，模拟Evidence/纯Engine或owned pipe测试不证明输入会话就绪。见 [ADR-0094](ADR-0094-reload-control-transaction.md)。

Linux 独立reload pipe端点/监督继承/真实main单次认领已实现，907项测试及无输入父子进程传输harness通过。当前真实worker/host尚未采用新模式，活动reload、watcher/CLI仍未连接；不宣称实际捕获热更新或Windows端点支持。见 [ADR-0093](ADR-0093-linux-reload-pipe-handoff.md)。

热配置冷任务与sealed actor交付桥已实现，9项新增portable测试通过（全套904）；每owner最多一job/Ready，Stop不重开/不抢占，actual activation先于ack，失效保持已激活代号不rollback。原生IPC、worker控制循环、CaptureSession借用adapter、watcher/reload仍待连接；Windows原生补验未执行。见 [ADR-0092](ADR-0092-cold-reload-preparation.md)。

此表描述已实现程序，设计文档中的 C1/C2/C3 是开发目标。离线支持不能证明系统后端、设备抓取或注入已实现。

Linux 已有实验 `run-linux` host/worker 冷启动与监督入口；当前 consumer 只支持显式文本 VT，`none` 永久禁止设备访问。图形桌面、普通 WSL2 终端及真实设备/停机验收尚未完成，见 [Linux 使用说明](LINUX_USAGE.zh-CN.md)。该入口没有放宽本表的配置语义 gate。

两平台冷 PreparedConfig 可指定非零 generation 或从已观察活动代号准备下一代，旧默认入口仍为1。配置准备不确认 actor 切换；旧持键使用旧 binding，新的 down 才使用已激活候选。跨进程热配置交付、确认与文件监听仍待接入，见 [ADR-0089](ADR-0089-prepared-config-generations.md)。

热配置独立请求/确认协议与actor当前代号查询已实现，确认不代表旧持键已释放；原生body交付、文件监听和reload命令仍未接通，见 [ADR-0090](ADR-0090-reload-control-protocol.md)。

独立热配置流已有有界header/body/ack读写与激活后确认门控；通用Read/Write未证明原生非阻塞和端点所有权，实际运行中的reload尚未连接。[ADR-0091](ADR-0091-bounded-reload-streams.md)。

| 项目 | 当前行为 |
| --- | --- |
| JSONC | 接受 `//`、`/* */`，保留源行号；8 MiB、serde 默认递归限额；拒绝尾逗号、重复对象成员（含 Unicode 转义等价键） |
| profile | 选择唯一 selected=true，否则第一个；检查所有 profile 的对象、selected/name 类型；只编译当前 profile，其余保留到以后重新编译 |
| simple / basic complex | HID keyboard、consumer、button；每阶段首匹配，无回流；complex 无 modifiers 时仅在没有修饰键输出 owner 时匹配 |
| 多动作 to | simple 1..=256、complex 0..=256 个键动作；前面的动作完成 down/up，最后默认保持到输入 up；repeat:false 使最后动作也立即轻触；中间 tap 的派生输出与 up 宏先结束；vk_none/空 complex to 无输出 |
| from.modifiers | mandatory/optional/any；八个 HID 左右键与 command/control/option/shift 组；组左优先，允许整组额外侧；optional 保留，mandatory 临时屏蔽；接受字符串/数组、别名去重；caps_lock/Fn 明确拒绝 |
| to.modifiers | 标准左右修饰与抽象组（左侧）；字符串/数组（最多 64 名），去重并按上游 modifier 枚举顺序；any 无动作；modifier/key 均进入下一阶段；普通主键 scope lazy up，末主键是 modifier 则 scope 持有到原 up |
| to_if_alone / to_after_key_up | 键与修饰作用域完整轻触；正常 up 先释放主动作，再 alone，再 after；alone 严格小于超时，新 key/button down 和非零 wheel 取消，move 不取消；lazy 恢复 down 时八个 HID 修饰键；拔除/Reset 取消宏 |
| to_if_held_down | 阈值先释放正常 to，再执行 held；最后输出持有至原 up；当前动作条件+原配置/设备；其他新 key/button 或同阶段派生 down 取消未触发任务，move/wheel/repeat 不取消；held 不自动取消 alone |
| to_delayed_action | complex/basic 的 to_if_invoked/to_if_canceled（对象或数组），默认 500ms；up 后继续等待；新 key/button 或同阶段派生 down 先执行 canceled；到期 invoked；两者单次完整轻触，不替换普通 to；保留旧配置/down 修饰快照与当前动作条件 |
| to.lazy | 布尔值，默认 false；八个标准修饰键 down/up 只更新逻辑 owners，后续普通 down/显式修饰/允许的指针事件/设备 neutral 刷新；非修饰键和变量赋值正常执行；下一阶段重映射采用其动作策略 |
| to.halt | 布尔值，默认 false；当前动作数组继续，已安排的 up 和变量 up 值完成；同一 rule binding 后续 held/alone/after/delayed 批次停止；被 conditions 过滤的动作无作用；各阶段 binding 独立 |
| pending 与取消 | up 后的任务保留 pending binding/cursor/reservation；最后任务完成回收；Reset/remove/代际替换直接撤销无宏；大取消宏+新 down 超预算或需回收 pending 容量时 CancellationPending，按 next_delayed 单独 CancelDelayed 后重试 |
| deadline 控制 | 显式 TimerFired{token}、Engine.next_deadline 时间/token；阈值时 timer 在普通物理事件前，Reset/remove 优先取消；提前/乱序拒绝，stale 无输出；晚交付按实际时间执行；单次一个任务，ADR-0011 |
| 设备归零 | 已知物理 up 后该设备不再有 binding 时提交 pending modifier 状态；未知 up 不推断归零；拔除结束后也刷新；不等同于 OS lost-input/submitted 故障恢复 |
| basic timing parameters | 四个已知时间参数整数 0..=86400000ms；默认 alone=1000ms，支持规则 override；alone、held、delayed 有执行行为（held/delayed 默认 500ms），simultaneous 参数保存待规则字段实现 |
| 持键更新 | 已持键完成旧输出，之后的 down 用新 generation；Repeat 不重新执行宏 |
| 多来源相同输出 | owner 引用集合，防止他人持键被中间 tap/up 打断；ADR-0002 说明上游兼容差异 |
| 设备代际 | 每个 id 仅保留最高 generation/removed；旧移除幂等忽略，旧 key/wheel/move 拒绝；4096 身份上限，Reset 保留历史；ADR-0005 描述容量恢复契约 |
| 应用条件 | frontmost_application_if/unless；bundle_identifiers/file_paths regex_search OR；没有标题匹配；provider 属性缺失/非 ASCII/CRLF/过期 Unknown |
| 输入源条件 | input_source_if/unless；input_sources 字段 language/input_source_id/input_mode_id AND、条目 OR；空 specifier 在已知源对象中 True |
| 变量条件 | variable_if/unless；严格 i64/bool/String 相等；未设置/unset 为整数 0；引擎本地状态不受 ContextSnapshot 过期影响 |
| 变量动作 | complex to/alone/after 中 set_variable，默认 type=set；value/key_up_value 至少一个，type=unset 不接受被忽略的 value；最后 repeat=true 的 up 值保留到原输入 up；非最后/extra/repeat=false 完整 down/up；没有按键 owner |
| 变量预算与恢复 | 256 names、256 bytes/name、4096 bytes/string；构造/激活预留，跨热更新保留，Reset 清值并回收旧配置名字；移除完成已执行 action 的 up 值，保留普通持久赋值；全局按顺序覆盖，ADR-0010 |
| 正则 | 仅 ASCII 单行匹配；配置时预编译，最多 256 patterns/当前配置、1024 bytes/pattern；32 KiB 程序/16 KiB DFA cache；拒绝 lookaround/backreference/inline flags/Unicode 或 hex 转义、集合运算、嵌套字符类/类内 shorthand；不是完整 C++ ECMAScript，ADR-0009 |
| 设备条件 | device_if/unless/exists_if/exists_unless；identifier 字段 AND、列表 OR；空对象忽略；规则数组全部 True；输出 action.conditions 在执行时筛选；未知/过期对 if/unless 均不激活，ADR-0008 记录上游差异 |
| 上下文输入 | ContextChanged：严格 revision、provider、[observed, valid_until) 纳秒；一天有效期上限；256 项设备库存、每 id 唯一；未知属性用 None；设备重连使 metadata 失效；离线合成、Linux captured-device metadata 已接入；Windows 应用路径 provider 代码已连接，原生验证待完成 |
| 未实现动作/条件 | 禁用整条外层 rule，保留 AST 与 JSON Pointer 诊断；validate/import/replay 不允许有损激活 |
| disabled rule | enabled=false 的内容保留，不编译、不执行；未知未来动作也不执行 |
| metadata | 根、profile、complex、外层 rule 允许 title/description/maintainers/authors/metadata；任意自定义数据放 metadata；不解释这些内容 |
| 其他功能字段 | 不静默忽略。未知根/profile/complex/rule 字段明确拒绝；非空 profile parameters/virtual_hid_keyboard、捕获 policy、未知 timing parameters 目前也拒绝 |
| macOS global | 当前明确拒绝整个 global 对象；后续区分 UI 元数据与平台功能设置，不能将其一概当无影响元数据 |
| 设备/Fn | 空数组接受；非空明确拒绝；结构错误直接拒绝快照 |
| 平台能力 | offline 支持上述 HID 页；linux-evdev 声明 keyboard/consumer；windows-hook 声明 keyboard。windows-hook 拒绝设备条件；两个后端尚无实现，声明只用于提前检查 |


尚未实现：simultaneous、expression/系统变量、hold_down_milliseconds、布局/锁状态、Linux 应用及两平台输入源 provider、Unicode 条件正则与会话动作。含不支持必要字段的规则准确拒绝；支持的条件没有可信 provider 值时保持 Unknown。Windows 应用 provider 代码的接入与未验证范围见 ADR-0074。所有时间为外部注入整数纳秒；同一时刻的输出以数组次序为准。wheel/move 当前只有离线 Pass、取消与受 repeat/button 状态约束的修饰刷新，没有指针捕获/映射后端。隐式 mandatory/scope 修饰恢复见 ADR-0006/0007，显式 lazy/halt 的状态与预算见 ADR-0013。

展开限额、不同来源 tap 冲突见 ADR-0003；轻触/松开生命周期与预算见 ADR-0004；实现与测试状态见 STATUS。不要通过删除不支持字段生成看似可执行的导入结果。

上下文 JSON 原文不进入 CLI 默认输出。已持键的输出释放不重查条件；alone/after-up 的动作条件读取执行时上下文，但采用原 binding 保存的配置。条件过滤后最后一个有效输出决定是否保持按下。设备地址、location 等缺失时保持 Unknown，is_touch_bar 明确拒绝。ContextChanged 不代替可信系统 provider、session lease 或故障恢复。

变量浮点/超 i64/null/对象、expression/key_up_expression、set_variable 与另一个目标同置、非键 modifier scope、simple 非键目标都拒绝。动作 conditions 在同一 to 数组执行前整批筛选；变量赋值可影响随后阶段的规则匹配及后续物理事件。CLI fixture 可给 expected_variables（需 --assert），输出及变量断言失败消息不导出实际值。

held 展开、future macros/suppression/deadline token 在原 down 一并预留；超限/时间溢出在改变输出前拒绝。上下文更新可先于同刻 timer，以便使用新属性；普通 key/wheel/move 遇到未交付到期 timer 会返回 DeadlinePending，运行时先按顺序 fire，再提交原事件。纯 held 规则可以没有普通 to。

延迟任务 active+pending 总量受同一 4096 binding/16384 reservation 上限保护；设备 idle 后延迟宏完成会刷新 latent modifiers，此恢复选择见 ADR-0012。EngineState.pending_bindings 描述松开后等待的任务，0 时省略序列化；replay next_deadline 给出 at_ns/token。CancelDelayed 不可抢先绕过已到期 timer；未知旧 token 无输出。按住/延迟仅属于 complex/basic，直接构造的 simple timed IR 也拒绝。单对象动作的诊断 pointer 指向对象成员，数组才含 /索引。

变量 down 会同步逻辑修饰状态；lazy 不延迟赋值。输出边沿与 owner/token 预留独立计算，包含最多八个 pending modifier 边沿，超过 1024 在 down 前拒绝；配置编译也检查此上界。halt 不撤销已安排 deadline，回调仍结束其 binding 生命周期并回收预算。

Linux 原生转换组件已有 159 HID/native 对与 154 canonical codes，但未接入 validate/backend；粗粒度 linux-evdev 页声明不能证明任意 HID 输出可用。keyboard 音量、non_us_pound/clear 等五个 native 别名明确拒绝保真输出；canonical evdev 身份不证明原始 HID usage，可信 scan 需 driver/descriptor 核验。详细策略见 ADR-0019。尚无真实设备抓取/注入。

Windows 后端进展：已有 portable scan codec、默认 injected bypass、保存 down identity 的 normalizer，以及处理部分插入/显式停捕获恢复的 OutputPump。cfg Windows 原生 SendInput 已编写，已在宿主 Windows 固定 Rust 1.97.1 通过 x64/i686 编译/生产链接，x64/x86 SDK ABI oracle 通过；真机验证待执行；已有同步决策绑定和原生 HookSession 注册/消息循环/卸载代码；无完整可执行会话运行时，CLI Windows capability 仍是规划的配置检查范围，不代表可安装运行。特殊 scan/别名输出在 backend 验证中拒绝。SendInput 的全局状态不会随 backend Drop 清除；未知接受量保留 quarantine，未捕获物理键碰撞协调仍待实现（ADR-0032/0033）。

Windows SessionActor 已连接共享引擎与输出账本，ActorHandler 可交付同步 HookHandler；覆盖标准键捕获重发、持键热更新与显式 tick 推进时间任务。Engine 物理 Repeat 不重播宏；Windows actor 已按原来源当前保留末端发送一条 Repeat，不增加 owner或重播前缀/变量，热更新读旧 binding、held 到期先换目标。repeat:false/已结束/非键盘/标准修饰不重复；采用当下已 emitted 的修饰状态，不重建首次 shortcut scope（ADR-0042）；已有具体同 epoch SessionClock/SessionControl 与原生定时 poll_with_control 源码；Windows 实际等待未验证；已有 passive neutral 启动组件，已有映射前固定紧急组合；碰撞/监督与可执行启动尚未完成，不能视为可运行 Windows 后端（ADR-0035）。

Windows startup 现有 callback 外只读采样：visible WinSta0/input desktop/HOOKCONTROL 与 foreground integrity 访问检查、无 impersonation、254 VK high-bit aggregate、pre/post context 重验。返回 Held/IdleObserved，不自动启用；不是原子 physical neutral。CaptureSession 已实现 passive Hook barrier/启动与关闭事务：启动 2s 期限、采样前后全部 binding 为空才同步启用，正常 capture 不全 VK 扫描；错误/Quit 先 close Hook 后有限恢复，保存卸载/恢复诊断；Drop 仅停 Hook，不注入或保证清全局键。实际 Windows API 与可执行 host 验证继续待做（ADR-0037/0038）。


Windows CaptureSession 紧急退出：原 physical Ctrl+Alt+Shift+F12 左右任一持续 >=1s，injected/passive 不参与，重映射不可覆盖。检测与 actor 共用一次 fresh delivery time，紧急到期先于同刻 up、held/delayed timer 和 due output；无新输入也在 control 推进。poll 返回 Emergency 后已关闭 Hook 并有限恢复，调用者检查 shutdown_report 的卸载/恢复结果；失败或未知 receipt 不宣称已清按键。Hook 无可信逐事件 keyboard ID，因此此组合按合并流检测，允许跨键盘组成。线程停滞/Hook 移除/桌面切换不受该检测覆盖，独立监督与完整 host 仍未完成（ADR-0039）。


Windows 活动访问检查已接入 CaptureSession：100ms 到期在 callback 外仅复核桌面/访问权限与前台 integrity，零 GetAsyncKeyState 调用；成功按完成时刻续期 500ms lease，callback 与 control 同 delivery time 检查有效期。高 integrity、失去输入 desktop、API/close/metadata 错误或过期/迟到结果终止会话并先停 Hook 再恢复，正常成功检查保留持键绑定。query 期间 callback 已停止时，不再续期/推进 actor。metadata pre/post 不原子，变化至下一检查之间仍可发生访问竞态；参数不提供墙钟时限，不能检测 Hook 静默移除/线程卡死，完整 WTS/session disconnect 与独立监督仍待实现（ADR-0040）。


当前 Windows 编译证据：宿主 Windows 10 build 19045 / Rust 1.97.1 / MSVC 19.44.35229.0 / SDK 10.0.26100.0，x64/i686 各 workspace 432 tests、fmt/clippy、native_link 和12个离线回放通过，SDK x64/x86 compile-only oracle 通过。MSG 的 `_MAC` 保留字段已删除，Windows size48/28。生产符号链接不执行 Hook/SendInput；独立监督、完整可执行 runtime 与真实设备语义/故障仍待完成（ADR-0041；[固定版本证据](validation/windows-2026-10-09-pinned.json)）。


Windows 未归属输入保护（ADR-0043）：active会话在other injected、unsupported physical Pass或故障放行down/repeat时停止；OwnInjected仍bypass，marker+flag不是鉴权凭据。风险sticky标记独立于首个错误；发送重入后禁止后续批次再调用backend。若有submitted down或unknown，不发可能释放外部来源的up，ShutdownReport.untracked_input=true且Unresolved；已知空ledger则Stopped。故障导致的有效captured up不会凭空建立新down，仍可走正常有限恢复；fault后close前观察到额外旁路会再次标记风险。此层会使外部自动化/unsupported键终止当前active会话，不能视为已支持共存或已完成逐键碰撞；unhook后的输入、系统静默移除及close/recovery窗口仍需完整监督/协调和真机验证。


Windows WTS租约检查（ADR-0044）：本地caller与foreground所属session必须相同且非0/保留值，WTSSessionInfoEx level1/完整长度、Active+Unlock和有效LogonTime均要求。Context两次比较包含session identity；CaptureSession也比较startup基线，已有新active session观察不能续旧会话。锁定/断连/身份变更/未知/权限或服务失败停止并沿用close-first恢复或untracked Unresolved规则。WTS所有操作在callback外，租约100/500ms不是RPC时长保证；Windows10/11目标，Win7反转flags不支持。尚无WTS事件通知/独立监督，检查间隔与pre/post非原子竞态保留；不能宣称真实锁屏/RDP/注销验收通过。

Windows WTS通知在本session任何变更时停止，包括unlock/connect/desktop-ready，不能由消息授予或恢复lease。另session消息忽略；通知不认证发送者且有交付延迟。通知发生后保留非空/unknown submitted，禁止recovery新提交；通知前已进行的API仍按真实receipt确认，不宣称跨桌面原子性。没有独立监督或真机验收；metadata查询失败且尚无通知时也通过ADR-0046的独立锁存阻止恢复提交；没有跨桌面原子性。见ADR-0045/0046。

Windows metadata或短租约失效，即使WTS消息没有到达也阻止新恢复提交；known empty正常停止，nonempty/unknown submitted保留为Unresolved。later成功诊断不恢复旧lease；secondary probe错误不被首callback故障遮蔽。通知到达/观测返回之前的实际API仍按receipt确认，不能宣称无窗口的原子停止。见[ADR-0046](ADR-0046-windows-revoked-access-recovery.md)。

进度监督v1协议由共享crate统一，Linux阶段enum保持兼容别名及原字节值。Windows passive/Standby的实际poll会增加计数，但不回应进度；publish须看到challenge之后完成的poll，并且phase已进入Active/Recovering/Stopped。Stopped包含Unresolved，只表示事件循环停止，不证明全局按键已释放或进程退出。客户端拒绝callback IPC；generic streams必须由host提供真实非阻塞端点，当前未提供Windows native监督器。见ADR-0047。


Windows新增owned Job进程资源接口（ADR-0048），无设备/输入行为：挂起创建、private Job分配、显式恢复、停止请求和实际退出分别表达；JobSpawnError保留partial owner。双架构无输入原生例程已确认argv传递、259退出和own Job显式终止。它尚未接入进度监督或host；Create→Assign期间的父崩溃窗口、真实父死亡与Hook存活仍待实现/验收，不能据此宣称卡死/崩溃恢复已完成。


ADR-0049把Windows owned Job准入放入CreateProcessW扩展启动事务，无创建后Assign回退；创建成功时worker已在private Job内。Windows x64/i686实际无输入fixture已验证父被强制终止后worker退出（通过保留进程句柄观察，不靠Rust Drop/PID消失）。该结果不证明全局注入键清理、Hook存活或完整进度监督；native IPC/启动核验/host仍缺失。


Windows新增native overlapped pipe transport（ADR-0050）：私有单向byte pair、64-byte稳定buffer/单flight/不可继承句柄，与共享v1协议相容；pending不等于发送完成，changed pending write拒绝。显式取消只有event completion确认后才能回收；Drop未完成会保留资源并毒化线程，不承诺全部回收。双架构无输入原生例程验证same-process往返/EOF/cancel；fixture epochs不表示真实runtime活性，尚无child端句柄交付/启动鉴别或独立progress supervisor。

Linux Runtime已连接captured Probe vendor/product与device条件：current-device匹配使用完整DeviceInstanceId/generation，其他属性无证明保持Unknown。ContextSnapshot的device_inventory_scope=partial表示仅部分设备；已知存在可True，未匹配不足以证明不存在，exists_unless不变True。Complete继续旧完整inventory语义，缺字段只沿用旧provider契约；Linux runtime始终发Partial。1h有效期与半小时bounded刷新保持绝对时钟和原持键release，仍未证明全局inventory/热插拔/consumer-ready/真实设备验收。[ADR-0066](ADR-0066-linux-device-context.md)。

Linux runtime新增单实例remove_device控制：完整device/generation匹配，关闭该捕获后仅清其owner/timer/未交付buffer，其他设备同键仍持有输出；last-device/emergency loss进入原整体有限恢复。tombstone保持slot身份，不重排队列。自动检测、可信hotplug绑定和device add尚未实现，普通IO/ENODEV/HUP仍全局故障恢复；不能将控制事务等同自动热插拔。[ADR-0067](ADR-0067-linux-device-removal.md)。

Linux配置可通过host check-linux离线校验完整可执行性：Linux capability编译拒绝有损规则，所有动作/modifier须在指定output spec内，Engine gate保留，External repeat拒绝。默认全键表通过不证明真实输入覆盖、consumer readiness或可执行会话；PrintScreen使用Linux原生code而非Windows特殊scan gate。[ADR-0068](ADR-0068-linux-cold-preparation.md)。

Linux默认challenge协议与run_worker行为保留；新增receive_command/run_worker_controlled仅显式启用KWPS停止。父request_shutdown必须知道worker支持，否则到期限强制停止，不推断兼容。停止先关闭捕获、有限恢复；短写必须补REPORT，unknown acceptance保留错误。父不读取旧receipt续租，Stop写完不等于退出，Cooperative/StopReason不证明键盘恢复。真实Linux入口仍待接通。[ADR-0069](ADR-0069-linux-cooperative-stop.md)。

Linux新增TerminationReport与受控WorkerReport：原StopReason/outcome保持，明确destroy前pending REPORT、未知接受量、未释放submitted、恢复预算与错误，Engine reset和owner释放独立。报告不含按键或设备身份；进程内资源/接受量事实不能证明compositor或全局状态，当前无跨进程报告通道/真实Linux入口。[ADR-0070](ADR-0070-linux-termination-evidence.md)。

Linux终止报告使用显式第三管道与KWLT/v1严格32byte codec，完整reason与10事实mask，正run/reserved/一致性校验；原KWPC/KWPR与默认两管道模式保持。父仅actual exit后读取，完整frame+EOF才确认交付；Missing/Invalid/Pending不等于clean。真实主线程例程只证明protocol/ownership/EOF，不证明实际键盘恢复，真实Linux入口/host仍待接入。[ADR-0071](ADR-0071-linux-terminal-channel.md)。

Linux报告发送/读取调度现在有限：各100ms/64 attempts，IO前后验证时间，完整帧与迟到失败并存。受控worker不隐藏原错误；父退出后收取结果缓存，不重设deadline/续进度lease。旧手动poll API保持，真实main无输入例程使用新native发送/collect路径；真实capture/bootstrap/consumer-ready/full host仍未验收。[ADR-0072](ADR-0072-linux-bounded-terminal-finalization.md)。
