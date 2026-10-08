# ADR-0018：Linux 原生事件与 raw 同步边界

状态：编解码/raw frame 状态机已实现，FD/抓取/输出设备及运行时未实现。2026-10-08。

## 顺序与依据

D06 的基础交付依赖 D02/D04，完整高级配置才额外依赖 D05。当前规则引擎已有可用的离线基础行为，故同步推进 Linux 边界，避免整个系统后端等待 duration actor；D05 多阶段匹配/计时接入与 Windows/驱动/session/GUI 目标保留。此次没有启用真实抓取或安装程序。

核验 [Linux input event codes](https://docs.kernel.org/input/event-codes.html) 和 [uinput](https://docs.kernel.org/input/uinput.html) 官方文档，并读取本机 `/usr/include/linux/input.h` 与固定 libc 0.2.176 的 input_event 定义。EV_KEY value=0/1/2 对应松开/按下/重复；frame 用 SYN_REPORT 分界。SYN_DROPPED 后忽略至下一个 SYN_REPORT，并查询设备当前状态。uinput 输出的原生时间戳不作为引擎时钟。[event codes](https://docs.kernel.org/input/event-codes.html)、[uinput](https://docs.kernel.org/input/uinput.html)

## 实现选择

新增有行为的 keyweave-linux crate（无 unsafe）。Event 保存 native type/code/value 与 timeval；按 native endian 逐字节编解码，不对不对齐切片做 C struct 转换。显式 Time32/Time64 fixture 布局为 16/24 bytes。native_layout 当前只开放 x86_64/aarch64/riscv64 Linux 的 24-byte ABI，并检查编译 libc input_event 的 size/type/code/value offsets；其他 ABI 不自行猜测，Windows 仍能执行纯协议测试。

一次 decode 最多 1024 records、长度必须完整对齐，拒绝无效 timeval 和 time32 溢出；全批成功后再交给状态机，不交付部分有效前缀。FrameDecoder 保存跨读取的未完成帧，256 events 上限。未知 records/MSC/repeat 保留原数据，HID 转换和设备 capability 准入在后续层处理，不静默丢未知键。

选择 raw 路线，由 FrameDecoder 唯一负责 SYN_DROPPED 状态。部分帧先清除，发 Dropped；忽略到 report，发 Resynchronize；随后继续忽略，直到调用者完成内核查询/状态协调和 neutral gate 并明确 resynchronized。本地帧超限也进入相同恢复，禁止截断后输出部分按键。batch 超限在任何 decoder 状态变化前拒绝。它不凭掉队数据制造 down/up，恢复策略需运行时解除 grab、清 submitted 并等待 neutral。

输出编码最多 256 key edges，并添加一个 report；空数组不制造 frame。拒绝 reserved code 0、超 KEY_MAX 和非法 value。classify_write 只接受已知 record 对齐前缀，不将非对齐字节数伪装成 accepted edges。Incomplete 即使 accepted 等于全部 key edges，仍标 report_pending；调用者必须处理 frame flush/failure，不能只用边沿 ack 判定成功。接受数量模糊时需隔离/恢复物理输入，具体系统 write/errno 语义仍待 FD 实现核验。

## 接入与未完成事项

下一步 KEY_*↔HID 映射、严格设备 capability probe、EVIOCSCLOCKID/EVIOCGKEY 等窄边界、非阻塞 evdev 与 uinput create/emit/销毁、ready→neutral→grab 顺序、own-device 排除、单 repeat 来源、SYN_DROPPED 的真实回收和独立 watchdog。引擎时钟/seq 由 runtime 注入，不把未核验 native timeval 直接当作 MonoTime。没有事件流读写、实际 ioctl、FD 或系统抓取；codec 通过不证明任何硬件功能。

11 个新测试（10 个纯协议、1 个本机 ABI）覆盖 byte offsets、times、aligned bounded batch、frame 跨读取/未知数据/repeat、SYN_DROPPED/显式 resync、local overflow/拒绝原子性、output report、完整/短/模糊 write。Ubuntu 24.04.4 x86_64 / Rust 1.97.1，仅离线；其他 Linux 架构和 Windows CI 未执行。本机 ABI 检查是 libc layout 检查，未运行 C probe 或内核设备集成。
