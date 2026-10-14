# ADR-0208：Windows 显式鼠标输出编码与发送边界

新增纯数据 pointer 模块：Motion 为相对 dx/dy，Wheel 保留原 signed i32 的 DWORD 位模式、水平/垂直各一 packet，Button 只支持 HID button1..5 Down/Up，Repeat 与其他 usage 拒绝。每 packet 单一类别，time=0、非零 InjectionTag，不混合 wheel 与 XBUTTON mouseData，不加 absolute/virtualdesk。按原顺序有界编码，最大256；不换算120刻度、不合并、不排序、不自动重发。

独立 SendPointerBackend trait，现有 SendInputBackend 显式实现，完整 batch/tag 先验后打包 INPUT_MOUSE，复用一次同步 SendInput 与既有 accepted-prefix/last-error 语义。无自动注入或捕获，默认 capabilities/键盘 runtime 不拓宽。部分接受不能重放移动/滚轮，按钮恢复需后续 mixed ledger/pump；此接口尚未由 Runtime 调用。

资料：[MOUSEINPUT](https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-mouseinput)、[SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)。相对运动受 Windows 速度/加速设置影响，不可宣称原始设备位移恒等；SendInput count 不证明应用收到，UIPI 限制保持。真实鼠标 Hook 输入、绝对坐标转换策略、scope/ledger/pump 与原生 SDK/输入验收仍待完成。

测试纯编码、limits/unsupported、marker、原生 union 打包与 tag/bounds 预检；Linux mock reprC 不替代 Windows SDK/native invocation。
