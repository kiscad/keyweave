# ADR-0146：Simple consumed scope 接入保存后代模板

将 Simple 私有生成队列的 scope open 捕获槽转为 Complex 父模板引用；close 精确关闭该次 open 的引用。多个槽需作为一组处理：只在组开始检查 due，随后自身生成的控制不能阻止同组剩余引用准入；任何组内失败锁存 Failed，须 Reset，不宣称整组 rollback。

Simple 保存触发 scope 的设备/source/旧 Arc/槽列表和模板引用，生命周期独立于孩子动作完成；close receipt 核对应 open，并重试此前因 scope 未交付而延迟的孩子退休。原 Up 不重选规则，空匹配仍不造来源。

设备控制先清实际目标设备，再清触发设备的 scope：已随目标移除而失效的模板引用过滤；其他设备来源的引用通过 ADR0145 即时恢复，返回 intents 合入生命周期结果。其他触发 scope 只剔除失效目标引用，保原 close 凭据。Reset 清所有记录。由于公开 actor 仅提供有界能力，scope 数沿原 4096 batch 边界，原引用/queue/source 限制保留。

完成收发/跨设备清理及回归后撤 Simple mandatory modifier gate；Simple advanced branches 仍未接入 actor，native timing gate 保留。不宣称 native 平台输出或 OS 接受。

延后的 open 可能在来源设备移除后才交付。Simple 从 DeliveryLedger 的保留槽记录取消身份（包括已逻辑 Up、实际尚未 Up 的来源），与未交付 close 的 scope 来源集合取交集；未来 open 跳过这些已确认取消槽，close 仍核原完整槽 metadata。集合沿原 16384 scope 引用上界，结束/设备控制时剪枝，Reset 清空，禁止将未解释的缺失来源当作取消。

16 集成+2 故障测试及工作区 1344 用例通过。通用 Shift 原具体侧别匹配不变；同一 modifier 的多个来源全部投影。纯阶段 main consumed scope 已启用，高级 Simple 分支未启用。普通键/变量后代的冻结来源投影沿原 KeyWeave ownership 约定；本轮没有运行 mac 差分或真实输入验证，不能据此宣称完整 mac 运行时等价。
