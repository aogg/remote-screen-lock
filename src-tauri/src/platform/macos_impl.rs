//! macOS 平台实现（P1 正式支持，代码级兼容；tech-notes 1.2 结论）
//!
//! - 系统锁：优先执行系统自带 CGSession -suspend；文件缺失时降级 osascript 模拟 Ctrl+Cmd+Q
//! - 熄屏：`pmset displaysleepnow`（无需 root）
//! - 唤醒：`caffeinate -u -t 1`（声明用户活跃，点亮显示器）
//! - 防休：spawn `caffeinate -is` 子进程，退出时 kill

use std::process::{Child, Command};
use std::sync::Mutex;

/// CGSession 系统私有路径（macOS 大版本升级可能变化，运行时探测）
const CGSESSION_PATH: &str = "/System/Library/CoreServices/Menu Extras/User.menu/Contents/Resources/CGSession";

/// 防休子进程句柄（caffeinate -is）
static CAFFEINATE: Mutex<Option<Child>> = Mutex::new(None);

/// 执行外部命令并检查退出码，返回可读错误（R-04：系统命令失败返回可读信息）
fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    match cmd.status() {
        Ok(st) if st.success() => Ok(()),
        Ok(st) => Err(format!("{what} 退出码异常: {st}")),
        Err(e) => Err(format!("{what} 启动失败: {e}")),
    }
}

/// 系统锁：CGSession -suspend（快速用户切换式锁屏）；兜底 osascript
pub fn system_lock() -> Result<(), String> {
    if std::path::Path::new(CGSESSION_PATH).exists() {
        run(
            Command::new(CGSESSION_PATH).arg("-suspend"),
            "CGSession -suspend",
        )
    } else {
        // 兜底：AppleScript 模拟 Ctrl+Cmd+Q（首次会弹辅助功能授权）
        run(
            Command::new("osascript").args([
                "-e",
                "tell application \"System Events\" to keystroke \"q\" using {command down, control down}",
            ]),
            "osascript 锁屏",
        )
    }
}

/// 熄屏：pmset displaysleepnow
pub fn screen_off() -> Result<(), String> {
    run(Command::new("pmset").arg("displaysleepnow"), "pmset displaysleepnow")
}

/// 唤醒：caffeinate -u -t 1（声明用户活跃 1 秒点亮屏幕）
pub fn wake_display() -> Result<(), String> {
    run(
        Command::new("caffeinate").args(["-u", "-t", "1"]),
        "caffeinate -u",
    )
}

/// 防休开关：true=spawn `caffeinate -is`；false=kill 子进程
pub fn set_keep_awake(active: bool) {
    let mut guard = CAFFEINATE.lock().unwrap();
    if active {
        if guard.is_none() {
            match Command::new("caffeinate").args(["-i", "-s"]).spawn() {
                Ok(child) => *guard = Some(child),
                Err(e) => eprintln!("[platform] caffeinate 启动失败: {e}"),
            }
        }
    } else if let Some(mut child) = guard.take() {
        // 退出防休：杀掉 caffeinate 子进程（失败仅记录日志）
        if let Err(e) = child.kill() {
            eprintln!("[platform] caffeinate 结束失败: {e}");
        }
    }
}

/// macOS 无需专用防休线程（由 caffeinate 子进程承担）
pub fn start_keep_awake_thread() {}
