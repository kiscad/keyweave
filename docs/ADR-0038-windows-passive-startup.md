# ADR-0038：Windows passive Hook 启动与关闭事务

问题：单独的 IdleObserved 不证明可以捕获；安装期间已经放行的 down 必须保留其 Pass 到 up。ActorHandler 在 Standby 不能处理物理输入。

选择：同线程 CaptureSession 独占 Hook、actor 和共享 epoch。先配置预检和 callback 外采样，再安装仅 Pass 的 handler。启动限时 2s；每次有界消息 poll 后再采样，并检查采样前后 Hook 的全部 binding（含 unsupported Pass）。只有 IdleObserved、两次 binding 为空、无 callback fault 且未超时，才在不调用 OS 的连续步骤中启动 actor 并启用 handler。正常 Active 不进行全 VK 采样。所有 poll 错误或 Quit 先关闭 Hook，再进行 actor 最多 8 次恢复；保存卸载和恢复结果，不吞掉主错误。显式 shutdown 幂等；Drop 只停 Hook，不隐式 SendInput，不声称已清全局键状态。

替代：立即安装活动 handler 会破坏预先按住的按键生命周期；单独系统采样或 held_captured 不包括已放行的普通/特殊按键。等待固定 sleep 没有空闲证明。

边界：逐 VK 采样与 metadata pre/post 非原子；完整 tap 会继续放行，不能声称原子无损切换。session watchdog、保护组合、物理 Pass 与注入冲突、repeat 和可执行入口仍未完成，本组件不是可部署产品。测试替换 Win32 API；Windows SDK、链接与真实桌面验收独立进行。
