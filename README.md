# remote-screen-lock

一款运行在电脑上的开源**局域网远程锁屏 / 屏保工具**：电脑跑一个轻量"被控端"，手机在同一 Wi-Fi 内扫码打开网页控制台（免装 App），即可远程**锁屏、解锁、熄屏、唤醒**，并在**时钟 / 自定义图片 / 黑屏**三种屏保模式间切换。

- 零账号、零云端、零遥测：纯局域网通信，数据不出门
- 一个安装包：Release 页面任意时刻只有最新一个 Windows 安装包
- 既是工具也是屏保：锁定界面就是"值得展示"的时钟 / 相框
- 断外网可用：被控端与控制台全部资源本地打包，无任何 CDN / 外链

## 多端定位

| 端 | 形态 | 说明 |
| --- | --- | --- |
| **Windows 电脑** | 安装包（被控端） | 从 [Releases](../../releases/latest) 下载唯一的 `.exe` 安装即可（NSIS，免管理员） |
| **安卓 / iPhone 手机** | 网页控制台（遥控端） | 手机浏览器扫码即用，可"添加到主屏幕"当 App 用；**不是**锁手机自己的屏（安卓系统不允许第三方应用替换系统锁屏，手机端仅做遥控器） |
| **macOS 电脑** | 源码编译（被控端） | 代码已兼容（系统锁 `CGSession -suspend`、熄屏 `pmset`、防休 `caffeinate`）；安装包发布规划于二期（F-P1-01），届时 Release 为每平台保留一个安装包 |

## 快速开始（Windows）

1. 下载并安装 `remote-screen-lock_x.y.z_x64-setup.exe`（仅需普通用户权限）
2. 启动后系统托盘出现常驻图标；设置窗展示**二维码 / 8 位配对码 / 局域网地址**
3. 手机连接同一 Wi-Fi，扫码（或浏览器输入 `http://<电脑IP>:8326/pair` 输配对码）
4. 控制台四个大按钮：**立即锁屏 / 解锁 / 熄屏 / 唤醒**，状态每 3 秒自动刷新

> 首次使用请到 设置 → 安全 **设置本地解锁密钥**（4~6 位数字）；
> 未设置时锁定窗口仍可用快捷键 **Ctrl+Alt+Q** 直接解锁。

## 安全模型（请务必阅读）

- **系统锁边界**：设置中"系统锁"默认开启，远程锁屏会同时调用 `LockWorkStation`——此后**必须在本机输入系统登录密码**，这是操作系统的安全设计，任何正规软件都无法（也不应该）远程绕过。想远程完全解锁可关闭该开关（安全性降低，自担）。
- **HTTP 明文风险**：一期局域网明文通信，同网段攻击者理论上可嗅探 token。请在可信网络使用，建议路由器开启 AP 隔离；HTTPS 规划于 P2。
- token 是唯一凭证：怀疑泄露时在 设置 → 安全 → **重置 token**，旧手机立即失效。
- 密钥与配置：本地解锁密钥仅以 argon2id 哈希存储，配置文件不含明文。

## 功能一览（P0）

锁屏三模式（时钟 / 图片 / 黑屏）· 系统锁 · 远程熄屏 / 唤醒（熄屏期间自动阻止系统休眠保持在线）· 手机上传屏保图片（jpg/png/webp ≤10MB，总量 ≤50MB）· 首次扫码 / 配对码配对（10 分钟有效、一次性、5 次输错锁 5 分钟）· Bearer token 鉴权（同 IP 失败 10 次/分钟限速）· 端口占用自动顺延（8326~8335）· 托盘常驻（锁屏/解锁/熄屏/唤醒/设置，退出二次确认）· 断电重启不锁定、配置与配对不丢 · 多显示器（主屏内容、副屏纯黑）。

完整需求与验收标准见 [docs/prd.md](docs/prd.md)，接口与界面细节见 [docs/interaction-ui.md](docs/interaction-ui.md)。

## 项目结构

```text
remote-screen-lock/
├── .github/workflows/release.yml   # CI：Windows NSIS 单包发布（固定 latest tag）
├── frontend/                       # 零构建前端（纯静态，无 npm/框架/CDN）
│   ├── index.html + assets/console.js   # 手机控制台
│   ├── pair.html  + assets/pair.js      # 配对页（扫码/输码）
│   ├── lock.html  + assets/lock.js      # 锁定窗口（三模式）
│   ├── black.html                       # 副显示器纯黑页
│   ├── settings.html + assets/settings.js # 桌面设置窗
│   └── assets/manifest.webmanifest      # PWA manifest
└── src-tauri/                      # Rust 被控端（Tauri 2 + axum）
    ├── src/main.rs                 # 入口：装配/托盘/HTTP/监督任务
    ├── src/app_state.rs            # 状态机/配对/限速/日志/图片库
    ├── src/config.rs               # 配置持久化（JSON + argon2 密钥哈希）
    ├── src/lock.rs                 # 锁定窗口管理（多显示器）
    ├── src/platform/               # Windows / macOS 系统能力隔离
    ├── src/http_server/            # axum：REST API + 内嵌控制台静态页
    ├── src/commands.rs             # 桌面 IPC 命令（设置窗/锁定窗）
    └── tauri.conf.json
```

## 构建

### Windows（与 CI 一致）

前置：Rust stable（≥1.90）+ [Tauri 2 CLI](https://v2.tauri.app/) + WebView2 运行时（Win11 自带）。

```bash
cargo install tauri-cli --locked   # 或经 npm 使用 @tauri-apps/cli
cargo tauri build                  # 产物：src-tauri/target/release/bundle/nsis/*_x64-setup.exe
```

开发调试运行 `cargo tauri dev` 亦可（前端为纯静态文件，无需任何前端构建步骤）。

### macOS（源码编译，P1 提供安装包）

```bash
cargo tauri build                  # 需 Xcode Command Line Tools
# 产物：src-tauri/target/release/bundle/dmg/*.dmg（未签名/未公证）
```

macOS 首启若被 Gatekeeper 拦截：右键打开，或 `xattr -cr /Applications/remote-screen-lock.app`。

## 常见问题（FAQ）

**Q：安装/运行时 SmartScreen 提示"Windows 已保护你的电脑"？**
未签名开源软件的预期行为。点击"更多信息"→"仍要运行"。2024 年起 EV 证书也不再即时豁免 SmartScreen 信誉，签名方案视项目情况后续评估。

**Q：首次启动弹出"Windows 安全中心警报"（防火墙）？**
被控端需要监听局域网端口。请勾选"**专用网络**"允许（家庭网络通常为专用网络）。若误点取消，手机将无法连接：控制面板 → Windows Defender 防火墙 → 允许应用通过防火墙，勾选 remote-screen-lock 的"专用"。

**Q：熄屏后电脑休眠导致手机连不上？**
被控端在熄屏期间会主动阻止系统休眠；但若电源计划/组策略强制休眠仍可能失联，建议将系统设置为"插电时永不睡眠"。

**Q：电脑换了路由器 / IP 变了？**
手机重新扫码或手动输入新地址即可，token 不变。

**Q：公司电脑装了会被拦截吗？**
企业杀软/组策略可能拦截端口监听或屏幕电源命令，此时接口返回失败并在电脑端 设置→日志 可查原因；请与 IT 确认放行。

**Q：能锁我安卓手机的屏幕吗？**
不能。安卓系统不允许第三方应用替换系统锁屏；本项目里手机是"遥控器"。

## 许可

MIT
