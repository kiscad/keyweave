# ADR-0205：host X11 cold launch 与 paired sidecar 校验

PreparedLaunch允许X11 provider，先验证完整private argv，再定位worker及同目录keyweave-x11-probe sidecar；worker锚点也必须regular executable且无group/world write；helper必须非symlink regular executable，与worker同owner且自身无group/world write，直接目录无group/world write并归worker owner或root。规范路径写回argv。不做PATH搜索、不执行helper或打开设备。

这是本地paired provenance/权限校验，不是签名/内容摘要认证，不能证明所有祖先或消除check-to-exec race；部署仍须可信目录/账户。系统动态库root trust完全不变。

host从已验证LinuxArguments派生同output spec/scope，按StagedRelative冷编译同份文件bytes后seal；X11可watch并创建同owner reload lane，None仍无reload/device。非法spec/config在spawn前拒绝。公共CLI provider选择下一步接入；此commit只冷准备，不启动worker。

测试用非执行fixture worker/helper、实际config文件与sealed artifact，验证pair/权限/外路径拒绝、scope/spec/bytes绑定、watcher和private argv。没有input/uinput/display/session操作。
