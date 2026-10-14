# ADR-0182：独立 X11 只读探针进程

Xlib连接错误可能结束进程，不能在持有capture/output的worker直接调用。新增独立keyweave-x11-probe executable，仅接受显式本地display和owned event node的major/minor，查询XI2 Device Node/Enabled/use/classes，输出有界schema JSON。不抓取、不选事件、不查询按键/坐标/窗口标题、不改变设备属性；进程退出只能表示探针结果，不能表示消费者就绪或输出恢复。

libX11/libXi动态加载只用固定root-owned不可组/全局写路径，私有FFI和SDKallocator RAII；查询device/class计数及property长度有界。目标node核字符设备和设备号，不用名称/VID认领。只收属于目标node的enabled slave keyboard/pointer，未关联/unsupported/错误无成功输出。X11服务同步调用可能阻塞，下一步由owned child deadline/协议验证/kill-reap承载，未接capture gate。

当前SDK headers缺失、sandbox root库UID65534，真实连接/SDK ABI未验证；不会以手写C布局自证SDK。记录主源公共布局和Linux64显式限制；root-trust失败退出unsupported。既有logind session metadata必须另与结果合并，Wayland不复用XI2结果。完整consumer/host/worker继续。

来源Xorg libXi XInput2.h及XIQueryDevice/XIGetProperty公共接口；新private unsafe边界，不复制实现，不加外部依赖。发行前仍须原生SDK oracle/实际Xorg验收。
