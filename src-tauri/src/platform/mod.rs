//! 平台能力层：系统锁 / 熄屏 / 唤醒 / 阻止休眠。
//! 按操作系统以 `#[cfg]` 隔离实现（Windows 一期主力，macOS 代码级兼容 F-P1-01）。
//!
//! 动作映射（tech-notes 1.1/1.2 结论）：
//! | 远程指令   | Windows                                   | macOS                          |
//! | 系统锁     | LockWorkStation                           | CGSession -suspend（兜底 osascript Ctrl+Cmd+Q）|
//! | 熄屏       | SendMessageW(WM_SYSCOMMAND, SC_MONITORPOWER, 2) | pmset displaysleepnow   |
//! | 唤醒       | SC_MONITORPOWER -1 + SendInput 模拟鼠标移动     | caffeinate -u -t 1      |
//! | 防休（熄屏期间保持 HTTP 可达）| SetThreadExecutionState(专用线程) | caffeinate -is 子进程 |

#[cfg(target_os = "windows")]
pub mod windows_impl;
#[cfg(target_os = "windows")]
pub use windows_impl as os;

#[cfg(target_os = "macos")]
pub mod macos_impl;
#[cfg(target_os = "macos")]
pub use macos_impl as os;

/// 其他平台（Linux 为 P1-09）：全部动作返回 50301 可读错误，不 panic
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub mod os {
    /// 系统锁（未支持平台）
    pub fn system_lock() -> Result<(), String> {
        Err("当前平台暂不支持系统锁（Linux 支持规划于 P1）".into())
    }
    /// 熄屏（未支持平台）
    pub fn screen_off() -> Result<(), String> {
        Err("当前平台暂不支持熄屏".into())
    }
    /// 唤醒（未支持平台）
    pub fn wake_display() -> Result<(), String> {
        Err("当前平台暂不支持唤醒".into())
    }
    /// 防休开关（未支持平台：无操作）
    pub fn set_keep_awake(_active: bool) {}
    /// keep-awake 线程（未支持平台：无操作）
    pub fn start_keep_awake_thread() {}
}

// —— 统一的 Windows/macOS keep-awake 会话句柄 ——
// Windows：SetThreadExecutionState 是线程级状态，由 platform 内的专用线程持有；
// macOS：caffeinate 子进程。两端都通过 lock/unlock 时的开关控制，见 app_state。
