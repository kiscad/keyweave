# ADR-0088：Linux 原生配对时钟连续性检查

日期：2026-10-10。状态：检测组件已接入；完整暂停通知与真机验证待完成。

## 问题与决策

当前原生 event/deadline 都在 CLOCK_MONOTONIC 域。它不包含系统 suspend 时间，恢复后旧短期授权可能仍未过期。CLOCK_BOOTTIME 包含 suspend 时间；两者行为由 [Linux clock_gettime 文档](https://www.man7.org/linux/man-pages/man2/clock_gettime.2.html) 定义。不要把 BOOTTIME 当成 evdev MONOTONIC 时间戳，也不要将暂停差额伪装为 worker progress。

每次 MonotonicClock.now 用相同固定顺序读取 MONOTONIC-before、BOOTTIME、MONOTONIC-after，三个独立同步查询，没有后台线程或系统时钟修改。返回原 MONOTONIC-after。private ioctl clock helper 只允许两个固定 ID，沿用 timespec 长度/转换/溢出检查。

## 证据与锁存

有效 bracket 必须 after≥before、与上次 mono/boot 非递减，并宽度≤10ms。真实同时刻 BOOTTIME-MONOTONIC offset 落在 signed `[boot-after, boot-before]`，与此前所有有效区间求交。signed i128 计算支持独立时间 namespace offset 导致的负差，不假设 BOOTTIME 一定数值更大。

不相交、倒退、过宽或系统调用失败永久锁存本 clock 的故障。last accepted MonoTime 不变，后续 now 不做任何新 native 采样，保留原 error kind/native errno，没有重试、替代时钟或 PID/time fallback。首次有效 sample 只是建立 baseline，不推断运行前的暂停。新建 clock 不能用来重新授权已有 runtime/provider。

原生 bootstrap→provider→worker 已传递同一个 MonotonicClock，因此后续 wait/provider 观察通过该检查；host 的同一个 spawn/control clock 也自动接入。既有错误路径仍关闭 capture、用 last Runtime 时间有限恢复；host control clock fault 请求 owned 强制停机。新建的 terminal-finalization clock 只用于 runtime owners 释放后的有限证据交付，不授权输入。wire/progress/evdev 时间域均不变。

## 限制

此证据不是系统 suspend event。重叠区间可能容纳短 offset 变化；两个各≤10ms 的区间最坏可有20ms歧义。三次读取并非原子，暂停若发生在 BOOTTIME 读取后或整个 sample 后，可能在下一次 sample 才被检测；尤其不能保证 sample→dispatch 区间没有暂停。

有些正常调度延迟也会使采样过宽并保守失败；这里没有硬实时 syscall 上界。不声称所有短暂停都被识别、也不自动清理物理键盘状态。暂停前通知/适当抑制握手、session/power 撤销、真实 suspend/resume 与输出释放测试仍需完成。此组件不能替代这些要求。

## 验证

见 [验证记录](validation/linux-2026-10-10-clock-continuity.json)。六 pure tests 覆盖稳定 offset/不同延迟/区间缩窄、MONOTONIC TTL 未过而 BOOTTIME 已变化、负 offset/相等时间、两时钟或 bracket 倒退/宽度边界、明确的 overlap 歧义，以及外部故障锁存。两个 native-clock 注入 tests 验 last time/errno 保留与后续 zero query/no fallback。

已有真实只读 clock/wait tests、无输入 bootstrap/worker/host fixtures 全部通过，未让机器 suspend/resume，未设置任何时钟，也没有 VT、evdev/uinput、Hook/SendInput 或驱动操作。C oracle 只核验 MONOTONIC=1/BOOTTIME=7 与 clock_gettime prototype，不调用 clocks/devices。

Linux workspace tests、Clippy、fmt、60上游来源和扩展 C ABI 检查通过，无新 dependency。Windows 既有原生/portable 待补检查仍未执行，Linux 检查不能替代它们。完整跨平台开发目标尚未完成。
