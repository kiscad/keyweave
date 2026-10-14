# ADR-0176：Linux 同一输出泵的相对事件屏障

键盘 accepted ledger 不包含相对值，不能把 move/wheel 当 Down 或在短写后重发。选择沿同一 OutputPump 增加有界 mixed FIFO：KeySpan 与 RelativeFrame；OutputQueue 新增 bounded begin_due_limited，使键盘 native batch 不跨 pointer 屏障。默认后端不声明 relative_axes，准入拒绝；实际 UinputDevice 能力设置与 Runtime 指针接入仍后续继续。

enqueue_mixed 先检查全部 key/relative 能力、总8192原生事件预算和 FIFO 时间，再一次提交键盘 desired 和 mixed 元数据。相对帧保持原轴值、报告和顺序，不生成按键 owner。所有键盘与相对时间沿同一尾时刻单调归一；next_deadline 以实际 FIFO 首工作为准。relative 不能越过未接受键盘，后面的键盘也不能跨 relative native report。

完整相对 receipt 消费该帧，不动 key ledger。aligned short 丢弃所有尚未发送工作，进入 REPORT pending，实际 report flush 后恢复已知 submitted keys；相对移动没有 Up/回滚，不重发余下 frame。known pre-write rejection 同样停止并恢复已知键；unknown/非法 receipt 冻结 ledger并 quarantine。用户 stop 丢未来 relative，destroy 先销毁唯一后端再清 unknown ledger。相对 ReportPending 与空 submitted 不构成已完成恢复。

证据要求：顺序/分批、归一时钟、完整/短/零/unknown/非法 receipt、report优先、stop/destroy、冷预检原子与总预算，及原键盘回归。无unsafe/FFI/依赖和真实设备操作。
