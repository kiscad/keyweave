# ADR-0222：Rust 原生配置编辑 GUI

落实规划中的keyweave-ui，固定eframe/egui 0.36.2（MSRV1.95，项目1.97满足），关闭默认features，选择default_fonts/glow/x11/wayland；不默认引入wgpu、持久化/网络或辅助捕获。来源：https://docs.rs/eframe/0.36.2/eframe/ 和 https://github.com/emilk/egui/releases 。Cargo.lock固定实际依赖，依赖获取与编译失败如实记录，不以旧workspace检查替代新crate验证。

GUI为普通用户独立进程，配置编辑/选择backend和mode、沿同worker PreparedConfig完整离线校验、展示结构化诊断与保存副本。不能因GUI存在启用session/input/capture，Apply/运行时控制在已有受监督IPC接入后再实现，不显示虚假已应用。文件打开保8MiB/UTF8限制，保存副本不覆盖现有文件；同目录临时文件完成sync后hard-link发布新路径，冲突/写失败保原文件，不弱化runtime artifact的权限/句柄边界。

egui/eframe原生窗口与headless GUI布局测试分别记录。原生Windows/Linux显示、中文字体/可访问性/托盘、实时session状态与导入/完整可视化编辑仍需后续验收；不能把headless帧当用户桌面运行成功。
