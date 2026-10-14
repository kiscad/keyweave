# ADR-0181：图形会话 sd-login 只读观察

图形消费者 gate 必须同时证明活动用户会话及真实输出消费者。先接入已存在libsystemd的sd-login只读会话观察；不调用TakeControl/TakeDevice、不把会话Active等同consumer-ready。

SessionSubject固定parent/worker/非root UID；SessionObserver两次同一subject query及前后注入时钟，要求session相同、UID匹配、active/nonremote/user/seat、x11或wayland。X11 display仅允许本地格式；Wayland无Xdisplay不伪造。初次eligible会话锚定session/UID/seat/kind/display，之后不自动换会话。TTL250ms从query前起算，clock倒退/overflow/late或不一致不留旧evidence；元数据race/ABA仍不能完全消除。

NativeSessionProbe通过已保护WorkerParentGuard绑定实际worker及effective UID；查询前后重验parent/isolated group/UID。动态打开固定架构路径下root-owned、非group/world writable libsystemd.so.0，私有函数表不导出handle/符号；SDK只有opaque/scalar API，返回malloc字符串有界复制后libc.free。错误保negative errno，不猜session或使用环境变量fallback。只控制面查询，不在input处理借用期间调用。

本机缺少SDK headers：实际符号及标量ABI核验不等于SDK header oracle或真实活动会话验收，必须独立记录。reader不建立capture token、消费者proof、窗口焦点/键盘权限或解锁证明。当前sandbox将系统库及根目录UID呈现为65534，root-trust检查拒绝加载；不放宽检查为任意UID，不以此环境宣称native exports/query已验证。本组件后续与真实X11/compositor consumer及mixed bootstrap接入，完整目标不缩减。

来源固定systemd v257 sd-login.h及官方man XML（API签名/allocator/negative errno）；无第三方源代码复制。新增private unsafe FFI边界，保持Linux条件编译和thread-affine library owner，无新依赖、真实设备或权限操作。
