//! 锁定窗口管理（F-P0-06 / F-P0-07 / README-appendix 1 多显示器默认策略）。
//!
//! - 主显示器：lock.html 渲染内容（clock/image/black 三模式）
//! - 其余显示器：black.html 纯黑置顶窗口
//! - 全部窗口：全屏 + 置顶 + 无边框 + 不进任务栏 + 不可关（CloseRequested 被拦截，
//!   阻断 Alt+F4 常见逃逸；Ctrl+Alt+Del 属系统安全桌面，无法也不应阻止，S-08）
//! - 窗口监督：锁定态下主窗口丢失（异常销毁）由 supervisor 重建（README-appendix 2）

use crate::app_state::AppState;
use crate::config::Mode;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

/// 锁定主窗口标签（内容屏）
pub const LOCK_MAIN_LABEL: &str = "lock-main";
/// 锁定副窗口标签前缀（纯黑屏）
const LOCK_SUB_PREFIX: &str = "lock-sub-";

/// 打开锁定窗口组：每个显示器一个全屏置顶窗口
/// image_path：image 模式时使用中图片的绝对路径（页面经 asset 协议加载）
pub fn open_lock_windows(app: &AppHandle, mode: Mode, image_path: Option<&std::path::Path>) -> Result<(), String> {
    // ////////////////  准备数据  start  //////////////////////////////////////
    // 先清理旧窗口（幂等：重复 lock 重建）
    close_lock_windows(app);
    let monitors = collect_monitors(app);
    let primary_pos = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| (*m.position(), *m.size()));
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    let mut main_built = false;
    for (idx, monitor) in monitors.iter().enumerate() {
        let pos = *monitor.position();
        let size = *monitor.size();
        let sf = monitor.scale_factor();
        // 判断是否主显示器（位置+尺寸一致）
        let is_primary = primary_pos
            .map(|(pp, ps)| pp == pos && ps == size)
            .unwrap_or(idx == 0);
        let (label, page, focus) = if is_primary && !main_built {
            (LOCK_MAIN_LABEL.to_string(), "lock.html", true)
        } else {
            (format!("{LOCK_SUB_PREFIX}{idx}"), "black.html", false)
        };
        // 物理坐标 → 逻辑坐标（builder 使用逻辑像素）
        let lx = pos.x as f64 / sf;
        let ly = pos.y as f64 / sf;
        let lw = size.width as f64 / sf;
        let lh = size.height as f64 / sf;
        let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page.into()))
            .title("remote-screen-lock")
            .decorations(false)           // 无边框
            .resizable(false)
            .maximizable(false)
            .closable(false)              // 关闭按钮不可用（CloseRequested 仍在全局处再拦一道）
            .always_on_top(true)          // 置顶
            .skip_taskbar(true)           // 不进任务栏
            .fullscreen(true)             // 全屏覆盖
            .focused(focus)
            .visible(true)
            .content_protected(true)      // 防截屏（Win/macOS）
            .inner_size(lw, lh)
            .position(lx, ly);
        // macOS：所有空间可见（Windows 无此概念，条件编译隔离）
        #[cfg(target_os = "macos")]
        let builder = builder.visible_on_all_workspaces(true);
        match builder.build() {
            Ok(win) => {
                if is_primary && !main_built {
                    main_built = true;
                    // 主窗口就绪后注入初始渲染参数（页面加载时也会主动 invoke 拉取）
                    let _ = win.eval(&format!(
                        "window.__RSL_INIT__ = {};",
                        serde_json::json!({ "mode": mode.as_str(), "imagePath": image_path })
                    ));
                }
            }
            Err(e) => return Err(format!("创建锁定窗口 {label} 失败: {e}")),
        }
    }
    if monitors.is_empty() {
        // 枚举失败兜底：单窗口铺主屏
        let builder = WebviewWindowBuilder::new(app, LOCK_MAIN_LABEL, WebviewUrl::App("lock.html".into()))
            .title("remote-screen-lock")
            .decorations(false)
            .resizable(false)
            .maximizable(false)
            .closable(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .fullscreen(true)
            .focused(true)
            .visible(true)
            .content_protected(true);
        builder
            .build()
            .map_err(|e| format!("创建锁定窗口失败: {e}"))?;
    }
    Ok(())
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 关闭全部锁定窗口（unlock / 重建前清理）
pub fn close_lock_windows(app: &AppHandle) {
    for (label, win) in app.webview_windows() {
        if label == LOCK_MAIN_LABEL || label.starts_with(LOCK_SUB_PREFIX) {
            // destroy 绕过 CloseRequested 拦截直接销毁
            let _ = win.destroy();
        }
    }
}

/// 收集显示器列表（去重：某些运行时会同时报重复项）
fn collect_monitors(app: &AppHandle) -> Vec<tauri::window::Monitor> {
    let mut list: Vec<tauri::window::Monitor> = app.available_monitors().unwrap_or_default();
    // 去重（按 position+size）
    let mut seen: Vec<(tauri::PhysicalPosition<i32>, tauri::PhysicalSize<u32>)> = Vec::new();
    list.retain(|m| {
        let key = (*m.position(), *m.size());
        if seen.contains(&key) {
            false
        } else {
            seen.push(key);
            true
        }
    });
    list
}

/// 锁定窗口监督任务（README-appendix 2：窗口异常丢失 5 秒内重建）
/// 由 main 在启动时拉起：锁定态下 lock-main 不存在 → 重建窗口组
pub fn start_supervisor(state: std::sync::Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tick.tick().await;
            let (locked, mode) = {
                let inner = state.inner.lock().unwrap();
                (inner.locked, inner.locked_mode)
            };
            if locked && state.app.get_webview_window(LOCK_MAIN_LABEL).is_none() {
                let img = state.active_image_path();
                match open_lock_windows(&state.app, mode, img.as_deref()) {
                    Ok(()) => state.log("system", "supervisor", "ok:lock-window-rebuilt"),
                    Err(e) => {
                        // 连续重建失败则回退 active，绝不把用户锁在黑屏里（README-appendix 2）
                        state.log("system", "supervisor", &format!("err:rebuild({e})"));
                        state.unlock_now("supervisor");
                    }
                }
            }
        }
    });
}
