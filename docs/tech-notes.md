# 技术调研笔记：Rust 多端远程锁屏/屏保应用（Tauri 2 + axum）

> 调研日期：2026-10-22。角色：技术调研（只出文档，不写项目代码）。
> 结论供 code 角色直接采用。所有"当前版本"信息均来自官方文档站/docs.rs 于本日抓取的结果。

## 0. 结论速览（TL;DR）

| 问题 | 结论 |
|---|---|
| Tauri 2 做桌面锁屏/屏保（Win/macOS）是否可行 | **可行，建议采用**。Win 用 `LockWorkStation` + `SetThreadExecutionState` + `SC_MONITORPOWER`；macOS 用 `CGSession -suspend`（锁屏）+ `pmset displaysleepnow`（熄屏）+ `caffeinate`（防休），全部有现成 Rust 调用路径 |
| 全屏置顶/阻断 Win 键/Alt+Tab | 一期做"全屏 + 置顶 + 隐藏光标 + 锁屏"即可满足安全边界；`WH_KEYBOARD_LL` 低级键盘钩子做成**可选增强开关**（有超时被系统摘除、杀软误报、无法拦 Ctrl+Alt+Del 等风险） |
| CI 打包 | `tauri-action@v1` 矩阵：windows-latest 出 NSIS（只留一个 exe，不出 MSI）；macos-latest 出 dmg（推荐 `universal-apple-darwin` 出一个通用包） |
| "只保留一个安装包" | 固定 tag（如 `latest`），发布前先 `gh release delete latest --cleanup-tag --yes`（runner 预装 gh），或"构建产物 upload-artifact → 末尾单独 job 统一删除旧 release 再发布"（**推荐**，避免矩阵内互踩）。softprops/action-gh-release 不清理旧 assets，**删除步骤无论如何不可省** |
| Android APK 一期上不上 | **不上**。手机端定位是浏览器远控台，网页即可；APK 需维护 gen/android 工程 + release keystore secrets，且 tauri-action 的 mobile 支持仍是实验性。二期若需要常驻/推送再上（runner 已预装 SDK/NDK，可行性没问题） |
| axum 内嵌静态 + PWA 同端口 | **可行**。`tower-http 0.7` 的 `ServeDir` 支持自定义 `Backend`（可接内嵌资源）；同一端口服务 API + 手机控制台。**注意**：局域网 `http://192.168.x.x` 非 Secure Context，Service Worker 注册不了——一期不做离线缓存，只发 manifest + 普通页面 |

---

## 1. Tauri 2 锁屏/屏保相关能力（2026 现状）

Tauri 本身（当前 tauri-utils 2.10.x，2026-10-14 发布）没有内置"锁屏"API，但它不封锁任何系统能力：在 Rust 后端直接调系统 API 即可。锁屏窗本体 = 一个 Tauri 窗口（全屏、无边框、置顶、隐藏任务栏图标），配合下述系统调用。

### 1.1 Windows（windows crate，当前 0.62.2）

依赖（crate `windows`，features 按模块开）：
- `Win32_System_Shutdown` → `LockWorkStation`
- `Win32_System_Power` → `SetThreadExecutionState`、`EXECUTION_STATE`、`ES_*`
- `Win32_UI_WindowsAndMessaging` → `SendMessageW`、`SC_MONITORPOWER`、`WM_SYSCOMMAND`、`SetWindowPos`、`HWND_TOPMOST`、`ShowCursor`、`SetWindowsHookExW`、`WH_KEYBOARD_LL` 等

#### (a) 锁屏：LockWorkStation
- 签名（0.62.2 实测文档）：`pub unsafe fn LockWorkStation() -> windows::core::Result<()>`，位于 `windows::Win32::System::Shutdown`。
- 效果等价 Win+L：回到登录界面，**不关显示器**。无参数、无需权限，失败返回 Err。
- 建议：这是 Windows 上唯一"官方"锁屏入口，远程锁屏命令应最终落到这里（安全边界由系统保证，比自制全屏黑窗更可信）。

#### (b) 防休/防熄屏：SetThreadExecutionState
- 签名：`pub unsafe fn SetThreadExecutionState(esflags: EXECUTION_STATE) -> EXECUTION_STATE`，位于 `windows::Win32::System::Power`。
- 常量同模块：`ES_CONTINUOUS`、`ES_SYSTEM_REQUIRED`、`ES_DISPLAY_REQUIRED`、`ES_AWAYMODE_REQUIRED`。
- 用法（被控端常驻在线时防止系统睡死导致手机连不上）：
  - 申请：`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)`（保系统 + 保亮屏；若允许熄屏只保系统则去掉 DISPLAY）
  - 释放：`SetThreadExecutionState(ES_CONTINUOUS)`
- 注意：这是**线程级**状态，调用线程退出即失效；应在被控端主循环所在线程申请，或改用 `PowerCreateRequest/PowerSetRequest`（Power API，更现代，windows crate 同样有绑定）。一期用 SetThreadExecutionState 足够。

#### (c) 关闭显示器：SC_MONITORPOWER
- 常量：`pub const SC_MONITORPOWER: u32 = 61808`（0xF170），位于 `Win32::UI::WindowsAndMessaging`。
- 发送：`SendMessageW(hwnd, WM_SYSCOMMAND, Some(SC_MONITORPOWER as usize), Some(2))`
  - lParam：`2` = 关闭，`1` = 低功耗，`-1`（即 0xFFFFFFFF）= 打开。
  - hwnd 可用广播句柄 `HWND(0xFFFF as _)`（HWND_BROADCAST）或应用自身窗口。
- 0.62.2 的 `SendMessageW` 签名（注意是 `Option<WPARAM>`/`Option<LPARAM>`）：
  ```rust
  unsafe fn SendMessageW(hwnd: HWND, msg: u32, wparam: Option<WPARAM>, lparam: Option<LPARAM>) -> LRESULT
  ```
- 风险：是否真正断电取决于显卡驱动/外接显示器；笔记本外接屏、部分 HDMI 显示器会显示"无信号"而非断电。锁屏场景建议顺序 = 先 `LockWorkStation` 再 `SC_MONITORPOWER`（关屏后动鼠标会亮屏并要求登录）。

#### (d) 组合建议（Windows 动作映射）
| 远程指令 | 实现 |
|---|---|
| 立即锁屏 | `LockWorkStation()` |
| 锁屏 + 熄屏 | `LockWorkStation()` → `SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, 2)` |
| 进入"屏保"（自制全屏画面） | 显示 Tauri 全屏置顶窗口（见 1.3） |
| 解除屏保 | 关窗口 + `SC_MONITORPOWER` lParam=`-1` |
| 保持在线防休 | `SetThreadExecutionState(ES_CONTINUOUS \| ES_SYSTEM_REQUIRED)` |

### 1.2 macOS

#### (a) 锁屏
先澄清一个常见误传：**`CGSessionCopyCurrentDictionary` 本身不能锁屏**，它只读当前登录会话字典（如 `kCGSessionOnConsoleKey`、用户名等），适合用来**检测**当前是否在控制台会话/锁屏状态。真正"锁屏"按优先级：

1. **（推荐）执行系统自带 CGSession 二进制**（无需额外权限、无 Accessibility 授权弹窗）：
   ```rust
   std::process::Command::new(
     "/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession")
     .arg("-suspend").status()?;
   ```
   效果 = 快速用户切换式锁屏（回到登录界面）。注意：这是系统私有路径，非公开 API，macOS 大版本升级需回归测试；找不到文件时降级到方案 2。
2. （兜底）AppleScript 模拟 Ctrl+Cmd+Q：
   ```rust
   Command::new("osascript").args(["-e",
     "tell application \"System Events\" to keystroke \"q\" using {command down, control down}"])
   ```
   首次触发会请求"自动化/辅助功能"授权（系统弹窗），体验差一些，作为 fallback。
3. （不推荐）dlsym 调私有 `login.framework` 的 `SACScreenLockAuthImmediate`：随时会被系统版本破坏，且签名/公证审查风险高，不进一期。

**读取会话状态（含检测锁屏）**：`objc2-core-graphics 0.3.2`（2026-10-10 发布）在 `CGSession` feature 下有：
`pub extern "C-unwind" fn CGSessionCopyCurrentDictionary() -> Option<CFRetained<CFDictionary>>`。
注意：servo 的 `core-graphics 0.25` **没有** session 模块（已验证），要用 `objc2-core-graphics` + `objc2-core-foundation` 这套。dlsym 私有符号则用 `libc::dlsym`。

#### (b) 熄屏：pmset displaysleepnow
- `pmset displaysleepnow`：立刻熄屏。属 pmset 的"立即动作"类子命令，**无需 root**（修改设置才要 root）。Rust 里 `Command::new("pmset").arg("displaysleepnow")`。
- 不建议用 `pmset -a displaysleep N` 改用户设置（要 root 且污染用户偏好）。

#### (c) 防休：caffeinate
- 拉起子进程 `caffeinate -i -s -u -t <secs>` 或常驻 `caffeinate -is`（i=防空闲睡眠；s=防系统睡眠【仅 AC 电源有效】；d=防显示睡眠；u=声明用户活跃，默认 5 秒超时需配 -t）。被控端"在线模式"期间 spawn 一个 `caffeinate -is` 子进程、退出时 kill，是最省事且无权限弹窗的方案。
- 原生等价物是 IOKit 的 `IOPMAssertionCreateWithName`，但 Rust 无成熟维护的 IOKit 绑定（`io-kit-sys` 覆盖不全），一期不值得。验证断言是否生效用 `pmset -g assertions`（比 `ioreg` 直观；`ioreg` 检查电源注册表是备选）。

#### (d) macOS 动作映射
| 远程指令 | 实现 |
|---|---|
| 立即锁屏 | `CGSession -suspend`（fallback osascript Ctrl+Cmd+Q） |
| 锁屏 + 熄屏 | 锁屏 → `pmset displaysleepnow` |
| 自制屏保全屏窗 | Tauri 全屏置顶窗口（macOS 全屏注意 `visibleOnAllWorkspaces`，见 1.3） |
| 保持在线防休 | spawn `caffeinate -is` |

### 1.3 全屏置顶 / 输入阻断

**Tauri 窗口配置**（tauri.conf.json 的 `app.windows[]`，字段已对照 tauri-utils 2.10.2 `WindowConfig`）：
```jsonc
{
  "label": "lock",
  "fullscreen": true,          // 全屏
  "alwaysOnTop": true,         // 置顶（Win 底层即 SetWindowPos(HWND_TOPMOST)）
  "decorations": false,        // 无边框
  "skipTaskbar": true,         // 不出现在任务栏（Win/Linux）
  "resizable": false,
  "maximizable": false,
  "closable": false,
  "focus": true,
  "visibleOnAllWorkspaces": true, // macOS 所有空间可见（Win 不支持该字段，忽略）
  "contentProtected": true     // 防截屏（Win/macOS）
}
```
运行时切置顶/全屏：`WebviewWindow::set_always_on_top(true)`、`set_fullscreen(true)`（Rust 侧 API，无需 JS 权限配置）。

**Windows 强化（可选、按需）**：
- `SetWindowPos(hwnd, HWND_TOPMOST(约), 0,0,0,0, SWP_NOMOVE|SWP_NOSIZE|SWP_SHOWWINDOW)`：Tauri `alwaysOnTop` 已覆盖，一般无需手写；需要"压过任务栏"时可再加 `SetWindowPos` + `SWP_NOACTIVATE` 组合与定时器重新置顶。
- `ShowCursor(false)`：隐藏光标。注意它是**计数器语义**（内部 display counter ±1），必须与 `ShowCursor(true)` 严格配对，且只影响调用线程。全屏窗口 + CSS `cursor: none` 已够用，Win API 可不做。

**WH_KEYBOARD_LL 低级键盘钩子：可行性 = 可行但有风险，建议做成"可选增强"**
- 可拦：Win 键、Alt+Tab、Ctrl+Esc、Alt+F4（在回调里对 `KBDLLHOOKSTRUCT.vkCode` + 修饰键组合返回 1 吞掉，其余 `CallNextHookEx` 放行）。
- **拦不住**：Ctrl+Alt+Del（SAS 由 winlogon 在安全桌面处理，钩子根本收不到）；任务管理器可经 SAS 界面启动——只能靠全屏置顶窗口盖住 + `LockWorkStation` 已锁会话来兜底。
- 风险清单（务必写进 README）：
  1. 回调超时：钩子回调耗时超过注册表 `LowLevelHooksTimeout`（默认约几百 ms）会被系统直接跳过/摘除，回调里**绝不能有锁等待/IO/panic**（Rust `extern "system"` 回调 panic = UB，需极简实现）。
  2. 需要安装钩子的线程跑消息循环（LL 钩子在安装线程上下文回调），要专门起线程泵消息，退出时 `UnhookWindowsHookEx`。
  3. 会被部分杀软/EDR 标记为键盘记录器特征（全局键盘钩子），提高 SmartScreen/Defender 误报概率。
  4. 会影响无障碍输入法、远程桌面等场景。
- **结论**：一期默认路径 = `LockWorkStation`（真锁）+ 全屏置顶屏保窗（观感层）已构成安全边界；钩子做成设置里的"最大封锁模式"开关，默认关。

---

## 2. Tauri 2 + GitHub Actions 打包（2026 现状）

### 2.1 tauri-action 基本用法与矩阵

`tauri-apps/tauri-action@v1`：build + 自动建 GitHub Release + 传 assets。官方指南（2026 版）标准流水线 = `actions/checkout@v7` + `actions/setup-node@v6` + `dtolnay/rust-toolchain@stable`（macOS 装双 target）+ `swatinem/rust-cache@v2` + `tauri-action@v1`，`permissions: contents: write`。

本项目建议矩阵（不需要 Linux）：

```yaml
strategy:
  fail-fast: false
  matrix:
    include:
      - platform: macos-latest
        args: '--target universal-apple-darwin'   # 一个 dmg 双架构
      - platform: windows-latest
        args: ''                                   # NSIS 单个 setup.exe
```

- Windows：`tauri.conf.json` 设 `bundle.windows.targets: ["nsis"]`（只出 NSIS setup.exe）。理由：MSI 依赖 WiX v3 + 系统 VBSCRIPT 可选功能（微软已公布弃用时间线，未来系统可能默认关闭）；NSIS 还支持 `installMode: "currentUser"` per-user 安装（免 UAC 管理员），对个人小工具更友好。**"一个安装包"目标下明确不出 MSI。**
- macOS：双架构两条路：`--target aarch64-apple-darwin` + `--target x86_64-apple-darwin`（两个 dmg），或 `--target universal-apple-darwin`（一个 dmg，体积翻倍但分发简单）。要"每个平台只留一个包"→ **选 universal**。

### 2.2 固定 tag + 只保留一个安装包（现成方案）

关键事实：
- `gh release delete <tag> --cleanup-tag --yes`：删 release 同时删 tag（gh CLI 手册明确支持 `--cleanup-tag`；GitHub runner 预装 gh 2.x）。`|| true` 兜底首次无 release。
- tauri-action：`tagName` 指向已存在 release 时会**追加上传 assets 而不清理旧文件**；`softprops/action-gh-release` 同样不清旧 assets → **任何方案里"先删后发"都不可省**。
- 注意矩阵互踩：3 个平台 job 若各自先 delete 再 build，先完成的 job 建好 release 后可能被后跑完的 job 的 delete 步骤删掉。

**方案 A（单 job 简版）**：只有 1 个 build job 内跑多步（少见），发布前 `gh release delete latest --cleanup-tag --yes || true`，再 `tauri-action` 固定 `tagName: latest`、`releaseName`（固定 tag 时必填 releaseName）。

**方案 B（推荐，矩阵安全）**：
1. 各平台 job 只构建：tauri-action **不传 tagName/releaseId**（官方 README 明确：不传则只构建不上传），或直接 `cargo tauri build`；
2. 每个 job `actions/upload-artifact` 传 NSIS exe / dmg；
3. 末尾独立 `release` job（`needs: [win, mac]`）：
   ```yaml
   - uses: actions/download-artifact@v5
     - name: 清理旧 release（保证只保留一个安装包/版本）
       run: gh release delete latest --cleanup-tag --yes || true
       env: { GH_TOKEN: "${{ secrets.GITHUB_TOKEN }}" }
     - uses: softprops/action-gh-release@v2
       with:
         tag_name: latest
         name: Remote Screen Lock（最新版）
         files: dist/*
   ```
   优点：发布动作只发生一次、天然去重、产物全了才覆盖，不会出现半成品 release。

下载链接稳定形态：`https://github.com/<owner>/<repo>/releases/download/latest/<file>`（固定不变，方便 README 引用）。

### 2.3 Android APK 在 GH Actions 的可行性（结论：一期不上）

事实核查（2026-10）：
- tauri-action 有 `mobile: android` 输入，但 README 标注 **EXPERIMENTAL**，且系统依赖（SDK/NDK/JDK 环境变量）要自己保证。
- runner 环境（actions/runner-images Ubuntu 24.04，2026-10-12 镜像）已预装：Android SDK（platform 34–37）、NDK 27.3/29.0/30.0（默认 27.3，`ANDROID_HOME`/`ANDROID_NDK_HOME` 环境变量已设好）、JDK 17/21、Gradle 9.8 → **环境本身可行**，无需额外装 SDK。
- 还需：`rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android` + `tauri android init` 生成 `src-tauri/gen/android` 工程。
- **签名是硬门槛**：release APK 必须有 keystore 签名才能分发（debug 构建自动用 debug keystore 签，不能当正式包）。keystore 可以本地 `keytool -genkey` 自签（免费），但必须以 secrets 形式进 CI（`ANDROID_KEY_BASE64`/别名/密码），官方文档给了现成的 `keystore.properties` 生成步骤和 gradle `signingConfigs` 片段。keystore **必须**放 secrets，绝不能入库。

**结论：一期不建议上 APK**，理由：
1. 产品定位：手机是"控制端"，浏览器打开 `http://电脑IP:端口` 即可用，APK 无增量价值（锁屏/屏保是被控端桌面才有意义的能力）；
2. 成本：gen/android 工程入库与升级维护、签名 secrets、双架构（arm64/armv7）矩阵、tauri-action mobile 实验性支持；
3. "只保留一个安装包"策略会被 APK 复杂化。
二期若需要（离线可用、系统分享、常驻通知），环境已验证可行，按官方 Android signing 文档加 secrets 即可。

---

## 3. axum 静态文件 + PWA 内嵌并由同一端口服务的可行性

**结论：可行，建议架构如下。**

- Tauri 2 后端就是普通 Rust 进程，内置 tokio；`tauri::async_runtime::spawn` 里跑 axum server（`axum::serve` 绑 `0.0.0.0:port`）即可，与窗口事件循环互不阻塞。手机浏览器访问 `http://<电脑局域网IP>:<port>`。
- 静态资源内嵌进二进制（无外部文件依赖）：
  - 方案 1（推荐）：`RustEmbed`/`include_dir` 宏把 `dist/` 打进二进制，axum 按路径查出来返回；MIME 用 `mime_guess`；
  - 方案 2：`tower-http 0.7.1`（2026-08 发布）`ServeDir::with_backend(path, backend)`——0.7 新增 `Backend` trait，允许自定义非文件系统后端（内嵌 map 即可），还能白得目录索引/404 fallback 逻辑；
  - 方案 3（页面极少时）：`include_str!`/`include_bytes!` + 手写几条 route。
  - 同时保留 `ServeDir::new(dir)`（`tower-http` feature `fs`）作为 dev 模式直读磁盘的选项。
- 桌面锁屏窗的前端与手机控制台**共用同一个 axum Router**（API 路由共享）；桌面窗口的加载源二选一：
  - a. `tauri.conf.json` 里 `frontendDist` 指向构建产物（Tauri 内嵌协议，端口被占用也不影响窗口）——**推荐**；
  - b. 窗口 url 直接指 `http://127.0.0.1:port`（省一套构建配置，但端口冲突时窗口空白，需启动顺序保证）。
- **PWA 的硬限制（重要）**：Service Worker 注册要求 Secure Context（HTTPS 或 localhost）。局域网 `http://192.168.x.x` 下 `navigator.serviceWorker.register()` 直接不可用；Android Chrome 的"安装 PWA"横幅也因此不出现。manifest.json 可以正常加载，手机浏览器"手动添加到主屏幕"（快捷方式）仍可用。
  - 一期策略（建议）：不做离线 SW，只提供 manifest + 响应式页面 + 大按钮（锁屏/熄屏/屏保/解锁）；README 说明"添加到主屏幕"当 App 用；
  - 若将来要离线：内网自签 TLS（用户装根证书）或套 Tailscale/反代 HTTPS，代价高，不值当。
- 安全建议（写进设计与文档）：控制台至少加"首次配对码/固定 token"（URL query 或输入框校验），否则同局域网任意设备都能锁你屏幕；监听地址默认仅绑定局域网网卡而非 `0.0.0.0` 全开（可配置）。

---

## 4. 风险清单与缓解（文档需写清）

| # | 风险 | 表现 | 缓解 |
|---|---|---|---|
| 1 | Windows SmartScreen（未签名） | 下载 setup.exe 运行时弹"Windows 已保护你的电脑" | 文档写"更多信息 → 仍要运行"。注：2024 年起 EV 证书也不再即时豁免（微软取消 EV 特殊信誉），OV 证书也要养信誉；一期不买证书，纯文档引导。可选后续接 Azure Trusted Signing/OV 证书 |
| 2 | macOS 未公证（quarantine） | Gatekeeper 弹"无法验证开发者/来自身份不明的开发者" | 无付费 Apple 开发者账号（$99/年）无法公证。文档写"右键打开"或 `xattr -cr /Applications/App.app`。**务必**在 CI 里配置 ad-hoc 签名（`signingIdentity: "-"` / `APPLE_SIGNING_IDENTITY: "-"`），否则 Apple Silicon 从浏览器下载的包会直接显示"已损坏"，用户连右键打开的机会都没有（Tauri 官方指南明确建议） |
| 3 | Windows 防火墙首启弹窗 | 被控端监听端口，首次启动弹"Windows 安全中心警报"，用户点取消则手机连不上 | 文档截图说明勾选"专用网络"；NSIS 安装器可选加防火墙规则（`netsh advfirewall firewall add rule`，需 perMachine/管理员）；应用内检测端口不可达时给出指引 |
| 4 | macOS 应用防火墙 | 首次监听入站弹"是否允许传入网络连接" | 文档说明点"允许"；系统设置→网络→防火墙选项里手动放行 |
| 5 | 电脑休眠导致远控失联 | 手机连不上被控端 | 被控端常驻时 `SetThreadExecutionState` / `caffeinate -is`；文档说明"在线模式"与系统电源设置的取舍 |
| 6 | 键盘钩子被杀软误报 | 启用"最大封锁模式"时 Defender/杀软告警 | 默认关闭该模式；README 解释原理与开源可审计 |
| 7 | 局域网 IP 变化 | 手机扫码/输入的地址失效 | 被控端 UI 展示二维码 + 全部网卡 IP；可选 mDNS（`xxx.local`）二期 |
| 8 | 端口冲突 | axum 起不来或桌面窗白屏 | 换随机/可配端口；健康检查 + UI 报错 |
| 9 | macOS 私有 CGSession 路径变更 | 大版本升级后锁屏失效 | 运行时探测文件存在，缺失降级 osascript；CI 无法覆盖，列入手工回归清单 |
| 10 | 多显示器/键盘布局差异 | SC_MONITORPOWER 对外接屏无效、钩子吞键影响输入法 | 文档"已知限制"章节列明 |

---

## 5. 来源（调研引用）

- windows crate（0.62.2）：
  - [LockWorkStation](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/System/Shutdown/fn.LockWorkStation.html)
  - [SetThreadExecutionState](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/System/Power/fn.SetThreadExecutionState.html)、[ES_DISPLAY_REQUIRED](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/System/Power/constant.ES_DISPLAY_REQUIRED.html)
  - [SC_MONITORPOWER](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/WindowsAndMessaging/constant.SC_MONITORPOWER.html)、[SendMessageW](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/WindowsAndMessaging/fn.SendMessageW.html)
  - [SetWindowsHookExW](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/WindowsAndMessaging/fn.SetWindowsHookExW.html)、[WH_KEYBOARD_LL](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/WindowsAndMessaging/constant.WH_KEYBOARD_LL.html)
  - [ShowCursor](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/WindowsAndMessaging/fn.ShowCursor.html)、[HWND_TOPMOST](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/UI/WindowsAndMessaging/constant.HWND_TOPMOST.html)
- Microsoft Learn：[Using Hooks（SetWindowsHookEx/全局钩子/超时与释放）](https://learn.microsoft.com/en-us/windows/win32/winmsg/using-hooks)、[SmartScreen reputation（EV 不再即时豁免）](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/smartscreen-reputation)
- macOS：
  - [objc2-core-graphics 0.3.2：CGSessionCopyCurrentDictionary（feature CGSession）](https://docs.rs/objc2-core-graphics/latest/objc2_core_graphics/fn.CGSessionCopyCurrentDictionary.html)
  - [core-graphics 0.25.0（已无 session 模块）](https://docs.rs/core-graphics/latest/core_graphics/)
  - [pmset 手册（displaysleepnow / -g assertions）](https://ss64.com/mac/pmset.html)、[caffeinate 手册（-disu 选项）](https://ss64.com/mac/caffeinate.html)
- Tauri 2：
  - [Prerequisites（Android SDK/NDK/NDK_HOME/rustup android targets）](https://v2.tauri.app/start/prerequisites/)
  - [GitHub 发布流水线指南（2026 版示例，ad-hoc 签名建议）](https://v2.tauri.app/distribute/pipelines/github/)
  - [tauri-action README（inputs：tagName/releaseId/mobile 实验性等）](https://github.com/tauri-apps/tauri-action)
  - [tauri-utils WindowConfig 字段（fullscreen/alwaysOnTop/skipTaskbar…）](https://docs.rs/tauri-utils/latest/tauri_utils/config/struct.WindowConfig.html)
  - [Windows Installer（NSIS/MSI、VBSCRIPT 要求、安装模式）](https://v2.tauri.app/distribute/windows-installer/)
  - [Windows 代码签名（SmartScreen/信誉机制）](https://v2.tauri.app/distribute/sign/windows/)
  - [macOS 代码签名与公证（ad-hoc、CI secrets）](https://v2.tauri.app/distribute/sign/macos/)
  - [Android 代码签名（keystore、GitHub Actions secrets 步骤）](https://v2.tauri.app/distribute/sign/android/)
- CI/发布工具：
  - [gh release delete 手册（--cleanup-tag）](https://cli.github.com/manual/gh_release_delete)
  - [actions/runner-images Ubuntu 24.04 已装软件（Android SDK/NDK 27/29/30、JDK、Gradle）](https://github.com/actions/runner-images/blob/main/images/ubuntu/Ubuntu2404-Readme.md)
- Web 服务：
  - [tower-http 0.7.1 ServeDir（fs feature、with_backend 自定义后端）](https://docs.rs/tower-http/latest/tower_http/services/struct.ServeDir.html)
  - [MDN Service Worker API（安全上下文要求）](https://developer.mozilla.org/en-US/docs/Web/API/Service_Worker_API)
