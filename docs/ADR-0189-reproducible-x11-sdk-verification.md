# ADR-0189：固定官方 SDK 的离线 X11 ABI 核验

系统缺 X11 headers 不应长期用模型测试替代真实 SDK。固定官方发布 libX11 1.8.12、libXi 1.8.2、libXext 1.3.6、libXfixes 6.0.1、xorgproto 2024.1 的归档 URL/本次实际 SHA256，新增离线 prepare_x11_sdk.py，只从完整校验的归档提取 include/X11 下的 regular .h，保留原版权，不安装库、不执行 configure 或 package scripts。输出目录必须不存在，头文件路径、size、数量有界，拒绝链接/越界和冲突；生成逐头文件哈希记录。归档哈希固定下载所得字节，未宣称已做发布签名验证。

verify_x11_abi.py 支持重复 --include，C compiler 独立核验全部使用的 struct size/offset/constants，以及已用 SDK 函数签名。Rust 布局测试将实际 repr(C) size/offset 与相同 SDK 常量对照；不是 X11 运行测试。64-bit ABI 限制仍显式保留，loader root trust 不变。

归档来自 [X.org library releases](https://www.x.org/releases/individual/lib/) 和 [protocol releases](https://www.x.org/releases/individual/proto/)。默认离线命令：python3 tools/prepare_x11_sdk.py --archives <归档目录> --output <新include目录>；python3 tools/verify_x11_abi.py --include <新include目录>。联网下载需显式进行，此工具不联网。系统无 SDK 的默认 oracle 仍报失败，不能自动回退到伪造声明。

实际 C SDK 通过仅解除所固定 Linux64 headers 的布局/签名缺口；动态库归属/加载、导出函数运行、真实 server、consumer admission、driver 设置和硬件验收仍独立未证明。

libX11 与 xorgproto 同时发布不同版本 XKBgeom.h，清单显式排除 libX11 的这一非本探针依赖文件，采用 xorgproto 版本；其余不同字节冲突仍拒绝。未以静默覆盖掩盖 SDK 差异。
