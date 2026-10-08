# 来源与许可证

本项目的新代码使用 MIT（LICENSE）。第三方来源不能据此重新许可：

- `tests/fixtures/upstream/`：Karabiner-Elements `ec2fea8940f7254a748c052a890ee9db7113d9b1`，Unlicense；同目录保存原始 LICENSE 与 manifest。
- HID 键名来自同一基线 `src/share/types/momentary_switch_event_details/`（Unlicense）；数值来自 `vendor/vendor/include/pqrs/hid/usage.hpp`，Copyright Takayama Fumihiko 2020，Boost Software License 1.0，保存于 `licenses/BSL-1.0.txt`。
- Cargo 依赖保留自身许可证；正式分发时导出依赖许可证清单。未复制 DriverKit、Kanata、keyd 或 KMonad 实现代码。
