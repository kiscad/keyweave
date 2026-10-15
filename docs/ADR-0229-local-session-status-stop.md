# ADR-0229：公开本地会话状态与停止控制

问题：GUI目前只有离线编辑，无法连接现有受监督host。先实现真实Status/Stop控制，不建立GUI自行抓取输入的第二入口。

协议置于keyweave-progress独立session_control模块，KWCC/v1固定32字节请求、KWSC/v1固定64字节回复，严格类型/reserved/正request ID/状态/退出事实约束。Status不含事件、配置、路径、窗口标题或private watchdog run ID。每个server冷阶段独立CSPRNG生成public session ID；不是worker/progress/reload run ID。Stop必须带此前Status得知的同public session ID，错旧ID返回Stale且不停止。StopAccepted仅表示请求交付，不等于实际退出/恢复；Exited只来自同owned Supervisor实际退出，recovery事实独立。

Linux使用显式socket路径、owner-only目录与0600 Unix socket，SO_PEERCRED双向要求同euid。拒绝已有路径、非socket/非owner/private目录；不扫描/删除陌生或旧socket。unlink仅在owned dev/inode仍一致时执行，不误删被替换路径。目录/祖先布局依赖可信用户目录，peer credentials不等于跨用户broker授权；root host不能由普通用户GUI控制。无TCP/远程传输。

host先完成每轮监督/到期/实际exit观察，再处理公开IPC，Stop交下一轮同owned pump。server非阻塞，每poll至多1accept、最多4clients，每client至多1read/1write；请求必须固定帧+write EOF，额外字节拒绝，client固定1秒期限，慢字节不续期。client非阻塞connect/read/write，总期限不超过5秒，同uid验证在发任何请求前完成；同样只接受固定回复+EOF。输入/输出缓冲固定且小于128字节，IPC工作不能代替progress poll。错误客户端仅关闭自己；监听器致命错误进入host既有停止/观察流程，不能放弃worker owner。Drop与socket消失不证明worker已退出。

NativeSession新增显式用户停止队列，消费于现有pump时钟/Controller；与真实信号来源独立，不伪造SIGINT。Controller合作Stop幂等，不续原deadline、不制造progress epoch、不跳过真实exit/终止报告。host显式run-linux --control-socket启用，原默认行为保持；status-linux/stop-linux可独立从CLI使用。GUI现沿同client连接：默认不连接，明确Query后每200ms自动查询；显示最后观察的endpoint/age，Stop二次确认绑定endpoint+public session，超过2秒的旧观察不可Stop。ACK/连接断开不当作exit，UI关闭不终止host。headless/transport测试仍不等于桌面产品验收。Windows公开控制transport另接具owner-SYSTEM ACL的named pipe，不假称Linux socket已覆盖Windows。

验证必须包括codec坏帧、peer/path身份、partial/extra/慢client、旧session ID、owned unlink、真实Unix连接、真实Controller Stop/exit边界；无真实设备/Hook/输入注入。Linux socket SDK布局/标量加入现有C oracle。主线程信号安装/worker启动顺序保持，冷bind/RNG在输入启动前完成；active loop不做配置解析、磁盘IO或stdio。

协议独立public session ID只防止误作用于新会话，不是授权secret；授权仍是私有用户目录与双向SO_PEERCRED。SDK来源：[unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html)、[connect(2)](https://man7.org/linux/man-pages/man2/connect.2.html)；peer凭据为connect/listen时的凭据，不宣称持续认证任意credential-changing进程。现有C oracle新增sockaddr_un/ucred布局、socket选项及函数签名。socket pathname权限不作为跨平台保障；Windows需独立named pipe ACL实现。

当前沙箱socket创建实际EPERM。真实AF_UNIX的library/CLI/GUI测试作为显式ignored native gate，由tools/verify_session_control_native.py统一运行；默认回归的通过数与native跳过/实际运行分开记录，不能将ignore变成通过。无输入fixture的owned Supervisor停止队列在普通回归实际执行，报告仅synthetic/no-input。

除独立transport/client测试外，native gate构建明确的session-control-fixture：它只验证private startup/密封配置，读取但不回复challenge，随后接受Stop并发送synthetic no-input终止报告。实际host CLI→socket→原Supervisor→同owned child→actual exit→最终Status全链断言epoch始终0，证明公开查询没有充当进度。此fixture不构造Runtime、设备或捕获，不证明真实键盘恢复。host只在已确认exit后保留500ms最终状态服务窗；socket消失不能代替exit证明。GUI Start/Resume/Apply/reload、设备/event viewer及Windows transport仍待完成。

最终证据：普通workspace1938 passed、8 ignored；这些ignored项在获准AF_UNIX环境中以native gate实际8 passed（3library+4CLI/真实host+1GUIclient）。默认沙箱最初3个socket测试EPERM，不把ignore记成通过；lint初错已修。既有源码62/C UAPI（新增socket布局/函数）/X11 SDK/30regex参考均通过。GUI未执行桌面点击/显示或实际捕获；Windows公开transport及完整目标未完成。见验证记录和SESSION_CONTROL使用文档。
