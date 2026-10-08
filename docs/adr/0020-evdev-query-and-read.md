# ADR-0020：evdev 查询与 owned FD 读取

状态：只读设备接口和准入分析已实现；真实键盘测试、grab/uinput/runtime 未完成。2026-10-08。

## 原生接口与安全边界

依据 [Linux input UAPI](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/input.h) 核验 ID、name、capability/state bitmap、clock 请求；本机 C probe 比较 struct input_event/input_id 的大小、offset、unsigned long 和 ioctl 请求编码。此 probe 不调用 ioctl、不打开 input；产品仍为 Rust。

Linux crate 将 unsafe_code 从继承 forbid 改为 deny，只在私有 ioctl.rs 以 allow 开放一处 libc::ioctl。types/engine/config 等 workspace forbid 不变。请求仅为固定 evdev 操作，direction/type/编码长度与 buffer 长度匹配、4096 bytes 上限；borrowed fd 同步有效，缓冲区初始化且独占，标量/version 与 clock 用显式对齐存储，copy 不保留 pointer。errno 不被吞掉，不在底层无限重试 EINTR。libc::Ioctl 处理 GNU/musl 的请求参数类型差异；仅使用已审核 native_layout 的 64 位目标。

## FD 与查询

EvdevDevice::open 接受显式路径，canonicalize 后必须是 /dev/input/eventN；只读、O_NONBLOCK/O_CLOEXEC/O_NOFOLLOW，检查实际 fd 为 input major 13 字符设备，再核验 evdev protocol version。FD 由 struct 独占，不提供 Clone、原始 fd 导出或 grab。Drop 关闭文件；本次接口从未抓设备。

查询设备 ID、name 字节、event/key/MSC/LED/property 位图，以 fd 的 rdev 查询真实 sysfs input syspath，不靠文件名/设备名字判 own virtual。查询失败或短 bitmap 明确错误，不以空值伪造 neutral。当前 name 只允许内核有界/终止结果；不可因为名字相似就推断设备权限。EVIOCSCLOCKID 设置该 client 的 CLOCK_MONOTONIC，runtime 仍需统一 seq/实际交付时间，不让 native timestamp 绕过 due timer 顺序。

pressed_keys/led_state 提供状态查询，read_batch 做一次最多 1024 records 的读取。WouldBlock、Closed、Interrupted、device error 和坏 record 各自区分；无关键读取不访问其他设备。read_batch 的公共 Read 接口也检查错误实现返回超过 buffer 大小的 count，避免切片 panic。底层不执行循环、配置解析或日志记录。

## 准入分析

Bits 使用 64-bit native-endian words，保留 0..767 code，拒绝长度、越界/未知 padding，不能按 little-endian byte 位顺序猜其他架构。Capabilities.keyboard_issues 按当前合同检查 keyboard/consumer：event 仅 SYN/KEY/MSC/LED/REP，KEY 必须在已支持 canonical 表，MSC 仅 SCAN/TIMESTAMP，LED 仅 Num/Caps/Scroll，pointer/touch/sensor properties 明确拒绝。

这是保守的当前支持范围，未知键/混合接口还没有 native passthrough；后续扩展必须先完成 opaque/LED/repeat 支持，不能删除诊断就抓接口。该分析不建立完整 activation gate：还需具体 config 可达输出验证、own uinput syspath 排除、输出 ready、neutral drain/query 与 grab 前后协调。不能把一张 pressed_keys 空快照当作无竞态抓取证明。

## 验证与后续

16 个新增测试：native bitmap word/bit 边界与拒绝、所有支持 keys、mixed class/property/unknown keys/LED、empty keys、显式路径限制、bounded read/EOF/EAGAIN/EINTR/partial/坏 Read count、真实 /dev/null ENOTTY 与 ioctl 编码。另 tools/verify_linux_abi.py 在本机 cc 编译的小程序通过，与 C UAPI 核验；无 keyboard/uinput 操作。

Ubuntu 24.04.4 x86_64 / Rust 1.97.1。实际 input nodes 不可用，未执行 EvdevDevice::open 的真实设备成功路径、ioctl 状态查询/clock/read、其他架构或 Windows CI。下一步实现 uinput owned 输出、短写/report completion、输出准入和明确 ready/neutral/grab，再接入 loop/watchdog。完整 D05/Windows/session/GUI/driver 目标继续保留。
