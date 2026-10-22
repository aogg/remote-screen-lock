//! Windows 平台实现（windows crate 0.62，feature 见 Cargo.toml）
//!
//! 签名依据（2026-10 官方文档核实）：
//! - `LockWorkStation() -> windows::core::Result<()>`（Win32::System::Shutdown）
//! - `SendMessageW(hwnd, msg, Option<WPARAM>, Option<LPARAM>) -> LRESULT`
//! - `SetThreadExecutionState(EXECUTION_STATE) -> EXECUTION_STATE`（线程级）
//! - `SendInput(&[INPUT], cbsize: i32) -> u32`

use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Power::{
    SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SendMessageW, SC_MONITORPOWER, WM_SYSCOMMAND,
};

/// 防休开关（由 keep-awake 专用线程轮询消费）
static KEEP_AWAKE: AtomicBool = AtomicBool::new(false);

/// 系统锁：等价 Win+L，回到系统登录界面（不关显示器）
pub fn system_lock() -> Result<(), String> {
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    unsafe { LockWorkStation() }.map_err(|e| format!("LockWorkStation 失败: {e}"))
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 显示器电源控制：on=false 熄屏（lParam=2），on=true 点亮（lParam=-1）
fn monitor_power(on: bool) {
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    // HWND_BROADCAST = 0xFFFF，广播到所有顶层窗口（作用全部显示器）
    let hwnd = HWND(0xFFFF as *mut core::ffi::c_void);
    let lparam: isize = if on { -1 } else { 2 };
    unsafe {
        SendMessageW(
            hwnd,
            WM_SYSCOMMAND,
            Some(WPARAM(SC_MONITORPOWER as usize)),
            Some(LPARAM(lparam)),
        );
    }
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 熄屏：SC_MONITORPOWER=2（主机保持运行，HTTP 可达由 keep_awake 保证）
pub fn screen_off() -> Result<(), String> {
    monitor_power(false);
    Ok(())
}

/// 唤醒：先发 SC_MONITORPOWER=-1，再模拟 1px 鼠标相对移动（部分显示器只认输入事件）
pub fn wake_display() -> Result<(), String> {
    monitor_power(true);
    // ////////////////  准备数据  start  //////////////////////////////////////
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 1,
                dy: 1,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
    Ok(())
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 设置防休开关：true=熄屏期间阻止系统休眠（F-P0-11：连续熄屏 8 小时仍可唤醒）
pub fn set_keep_awake(active: bool) {
    KEEP_AWAKE.store(active, Ordering::Relaxed);
}

/// 启动 keep-awake 专用线程。
/// SetThreadExecutionState 为线程级状态（线程退出即失效），
/// 故由常驻线程持有并周期重申（每 30s），保证熄屏期间系统不挂起。
pub fn start_keep_awake_thread() {
    std::thread::Builder::new()
        .name("rsl-keep-awake".into())
        .spawn(|| {
            loop {
                let flags = if KEEP_AWAKE.load(Ordering::Relaxed) {
                    ES_CONTINUOUS | ES_SYSTEM_REQUIRED
                } else {
                    ES_CONTINUOUS
                };
                unsafe { SetThreadExecutionState(flags) };
                std::thread::sleep(std::time::Duration::from_secs(30));
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| eprintln!("[platform] keep-awake 线程启动失败: {e}"));
}
