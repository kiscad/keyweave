# ADR-0140：Output 生成/实际键投影联合事务

问题：Terminal 投影尚未更新目的生成 modifier view，也没有匹配生成 marker，不能安全接入 owned Output。

选择：Terminal begin/close 改为可独占 prepare/commit/abort，原API包事务。Output guard同时持有Terminal投影与Generated投影预备，全部通过后共同提交。生成视图在commit隐藏/恢复保存logical key slots；实际SourceToken仍在FIFO marker交付时变更。两个marker receipt精确比对id/方向/target batch/time/stage/source/device/旧Arc。var仍冷拒绝，不凭当前配置重选。

close可在target batch实际结束之后使用保留投影记录；完成target batch不提前丢失投影，实际ProjectionUp配对后才清记录。cancel/device/reset两侧释放引用与marker。Cold拒绝/drop不改变任一侧view/ID/ref/queue/clock；执行失配锁存Failed并需要Reset，不能证明OS回滚。

仍未接Simple/Complex来源后代自动继承、变量生成/实际暂存与完整scope；native gate不解除，所有原目标继续。

验证：5个集成回放覆盖生成commit/实际FIFO/lazy modifier、begin/close abort、旧绑定嵌套与reload、target实际退休后close、future数组以及device/Reset；4个私有故障测试覆盖Generated/Terminal各自full拒绝、close第二侧拒绝后同token重试、缺recipe锁存Failed与Reset。原Terminal投影回归仍通过。Linux workspace1280 cases / Clippy -D warnings / fmt / source60+fixtures通过；中间新增enum缺Simple match及编辑锚点错误在接线前修复。未进行native/硬件/Windows验证。见 [验证记录](validation/linux-2026-10-12-output-projection-transactions.json)。
