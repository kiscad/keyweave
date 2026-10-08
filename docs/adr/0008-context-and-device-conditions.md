# ADR-0008：有界有效期上下文与三值设备条件

状态：已实现离线设备切片；2026-10-08。

条件需要当前 device generation、provider、revision、observed time 和有效期，不能在引擎热路径查询 OS。ContextChanged 是确定性 control input，使用严格递增 revision；新 down 用当前条件，旧 binding 的输出 release 不重匹配。up/action 条件仍在执行时读取当前上下文，并使用旧配置保存的条件 IR。

有效期为 [observed_at_ns, valid_until_ns)，最长一天；provider 名最多 128 bytes，device 地址最多 256 bytes，最多 256 个当前设备、每个 id 一条、properties 的 None 表示 Unknown。devices=None 是不可用，Some([]) 是已知空列表。丢失属性不能替代为 vendor=0、false 或空地址。if/unless 对 Unknown 均不激活，符合开发指导 7.3；这与上游缺失 device properties 时 unless=true 的部分行为明确不同。

device_if/unless 检查当前精确 id/generation；device_exists_if/unless 检查已知库存。identifier 字段 AND、列表 OR，采用三值逻辑；任何明确匹配使 OR 为 True，明确不匹配的 AND 为 False。普通条件数组全部 True 才执行。空 identifier 对象按上游忽略；无有效 identifiers 在已知相关上下文中为 False。is_touch_bar 的 Apple 特性明确拒绝。

设备事件先更新连接状态再匹配：新代际替换旧 metadata 为 Unknown，不沿用旧 vendor/product；未宣布的实际输入设备添加 Unknown properties，列表到上限则使库存 Unknown。移除删除该 id 的相同/更旧代际；旧移除不能删除更高代际。这样缺失库存更新不能使 device_exists_unless 误变成真。provider 下一 revision 可重新给完整库存。

Windows Hook 没有可靠每事件 device identity，本阶段设备条件在该 backend 编译时整条禁用。此字段门槛对库存条件也暂采用保守策略；之后 RawInput 库存 provider 与事件 identity 能力应分开声明，不能靠时戳拼接来放开 device_if。

本阶段只有设备上下文，不宣称已实现应用、输入源、variables、session lease 或真实 provider。上下文原文不进入默认日志；设备地址也不成为 telemetry。effect/action executor 与 OS runtime 以后必须补权限、提交状态及 provider 租约验证。

实现与验证：types/context.rs、engine/conditions.rs 与 binding 匹配、config 条件编译；95 个 workspace tests 通过，其中新增 13 个覆盖过期、三值逻辑、库存满/未知、重连、stale/duplicate context 原子拒绝、热更新旧 up、动作筛选后的 scope、CLI 回放。context 验证失败不推进输入 seq/time；零长或已过期快照可提交但不会匹配。Reset 保留 revision/库存和设备历史，年龄仍按显式时间失效。相同 provider 也必须提高 revision；provider 改名不允许重用旧 revision。

来源：固定基线 src/share/manipulator/conditions/device.hpp（已纳入 source manifest）；upstream valid() 忽略无属性 definitions，类型、AND/OR 与 device_exists 判断经静态检查。没有将 macOS device properties 默认值直接当成跨平台已知事实。
