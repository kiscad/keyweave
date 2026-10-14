# ADR-0175：Linux 相对事件能力与输出帧

原生键盘 Runtime/DeviceSpec 目前拒绝相对输入，无法沿 stage pointer receipt 转发。下一步接入必须先有精确原生帧与能力合同，不能将 pointer 记成按键 owner 或静默丢弃高分辨率滚轮。

选择：新增 relative 模块，白名单 REL_X/Y、REL_WHEEL/HWHEEL、两种 HI_RES，显式 RelativeAxes 能力与经预检的 RelativeFrame。保留记录顺序、重复轴、零值、i32 全范围及传统/高分辨率共存，不合并、不单位换算。activity 仅报告是否存在非零 movement/scroll，供 Alone 资格判断；不是相对值转换或可信设备来源。

写出重用既有 input_event Layout/encode/classify_write，完整帧为 EV_REL records + 一个 SYN_REPORT；空帧零写。准入拒绝先于 writer 调用。一次 write 的 aligned prefix 和 REPORT 完成区分；errno/非对齐/超长返回保持 UnknownAcceptance，不默认零、不自动重发。相对输出没有可用 Up 回滚；之后 OutputPump 必须按实际 prefix、报告边界、键盘先后和 capture-stop 合同接入，不能重发 whole frame。

能力声明是冷 metadata，不创建 uinput/抓取设备。本切片不放宽 keyboard Runtime admission，uinput RELBIT 与 probe/consumer/readiness/native integration 后续继续。独立输出接口是现有键盘输出合同的扩展基础，不以 mock 宣称原生已可用。

来源：本机 /usr/include/linux/input-event-codes.h；Linux 官方 https://docs.kernel.org/input/event-codes.html 的 EV_REL/SYN_REPORT 与高分辨率120单位定义、https://docs.kernel.org/input/uinput.html。高分辨率与传统滚轮共存，不承诺每次传统事件对应整120单位。无新 unsafe/FFI/依赖。
