# ADR-0082：Linux host 冷启动身份、参数与 worker 定位

日期：2026-10-10。状态：资源组件已实现；Linux host 完整监督/控制事务仍待连接。

## 结果与调用顺序

新增 Linux RunIdentity、LinuxArguments.into_argv 和 host::locate_linux_worker，供下一步完整 host 使用。它们不启动 worker、不抓取键盘、不安装权限或驱动。修正非Windows run-windows 文本，去掉“Linux bootstrap 尚未连接”的过时描述；未开放 run-linux。

完整 host 应依次执行：可信 worker 定位→文件冷读取与 Linux PreparedConfig→检查启动取消→fresh RunIdentity→seal 原 bytes→再检查取消→构造 worker argv→Supervisor.spawn_reporting_config→独立监督/stop/actualExit/KWLT观察。冷文件、编译、RNG、参数和seal不能搬进active watchdog/input循环。

## RunIdentity

Linux专用 `run_identity::RunIdentity::generate` 使用getrandom，8字节、GRND_NONBLOCK、urandom源（不设置GRND_RANDOM）。必须返回完整8字节，负值立即保留errno，短/超长返回拒绝，不拼接partial entropy。按显式LE解码非零u64；零值最多4次完整重抽，任何RNG错误直接失败，不在EAGAIN/EINTR时等待或用PID、时间、计数器、文件/dev/urandom、地址fallback。

Linux man-pages描述GRND_NONBLOCK在未就绪时返回EAGAIN，小于等于256字节的初始化urandom请求通常完整返回，调用者仍必须检查返回值。[getrandom(2)](https://man7.org/linux/man-pages/man2/getrandom.2.html)

该flag/字节预算不是固定syscall wall-time保证。调用在冷阶段，用户Stop和独立启动预算仍须处理。旧内核/环境不支持时失败，不弱化身份来源。

类型无Clone/Copy，Debug只有类型名，不输出随机字节/id；getter只读。getter值仍可被caller手动复用，完整host须每次新的spawn事务重新generate。随机u64可能碰撞，不是跨用户鉴权、image认证、virtual device provenance或密钥；Linux自己的虚拟来源仍用owned input syspath验证，不凭run id识别输入。测试不做“两次一定不同”的概率断言。

## argv 与路径

`LinuxArguments.into_argv` 消费结构，先拒绝空/超过64输入，再构造固定位置OsString数组并用真实main同一严格parser验证；不会将整串交给shell。caller只将数组交给Command.args。保持nonzero parent/run、None永久无设备、console canonical VT、absolute/unique inputs与uinput分离等合约；构造顺序保持原设备顺序。

字节/空格/单引号/美元符号留在单独参数中，不做lossy字符串转换或自行quote。转换仍属于冷准备，会有bounded count的路径复制，不可放进活动loop。parser/builder只作lexical检查，不证明设备存在、capabilities、consumer/neutral或物理身份。

## 定位范围

Linux默认定位canonical host同目录的 `keyweave-worker`（不加.exe），显式developer image必须absolute。候选canonical、regular file且至少一个execute bit；默认symlink逃出host目录拒绝，显式已选可信目标可在外部目录。定位器没有PATH/shell搜索，也不执行image。

execute bit并不证明当前用户有效执行权限/ACL/mount策略，spawn仍可失败；canonical/metadata不是签名/原子image attestation，也不能消除check-to-exec race。部署必须有可信目录和明确image选择。文件内容/ELF与实际启动、FD/信号/child owner保护仍要由完整host/原生spawn处理，不能把定位成功当成worker身份准入。

## 验证

Linux Rust1.97.1：完整workspace all-targets843tests、Clippy -D warnings、fmt、60-source verifier、扩展getrandom C ABI oracle通过。C oracle只核验本机GNU头文件的GRND_NONBLOCK/prototype和原ABI，不调用RNG或设备。

新增4个RNG测试（LE/zero重抽、四次预算/系统错误、count完整性、实际内核RNG非零/Debug隐私），3个argv测试（固定位置/字面空格、zero identity/duplicate拒绝、实际RNG+非Unicode路径roundtrip且无process），3个owned /tmp定位测试（Linux sibling/显式、relative/非exec/目录/缺失拒绝、默认symlink escape）。测试创建不可复用的私有目录，不执行伪image，Drop只删其own目录。[验证记录](validation/linux-2026-10-10-host-preparation.json)

新增unsafe仅限8字节getrandom FFI，有独立SAFETY依据；依赖及Cargo.lock未变。未操作VT/evdev/uinput、Hook/SendInput或driver。两新portable argv测试仍需Windows本机补验，前序Windows native补验仍受此前自动审批额度故障影响，本轮未调用Windows。

下一步实现host own signal handler/child process group（保护worker不被用户Ctrl+C抢先终止）与实际独立Supervisor生命周期。保持控制TTY/SID；不得用setsid破坏console grant，也不得在parent guard建立后改credentials/exec/fork device owners。然后开放显式run-linux及no-input fixture host验收，真实有效console捕获仍需专用授权环境。

图形桌面、睡眠/会话撤销、完整规则、hotplug/add、LED、热配置、GUI、打包、Rust driver与companion审计等原目标继续未完成。
