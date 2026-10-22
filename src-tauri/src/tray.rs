//! 系统托盘（F-P0-01）：常驻图标 + 菜单（打开设置/立即锁屏/解锁/熄屏/唤醒/退出）。
//! 退出需二次确认（tauri-plugin-dialog 原生对话框，blocking_show 在独立线程执行避免主线程冻结）。

use crate::app_state::SharedState;
use crate::{lock, platform};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

/// 托盘/菜单项 id
const ID_SETTINGS: &str = "open-settings";
const ID_LOCK: &str = "lock";
const ID_UNLOCK: &str = "unlock";
const ID_SCREEN_OFF: &str = "screen-off";
const ID_SCREEN_ON: &str = "screen-on";
const ID_QUIT: &str = "quit";

/// 创建托盘（setup 阶段调用一次）
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    // ////////////////  准备数据：菜单  start  //////////////////////////////////
    let item_settings = MenuItem::with_id(app, ID_SETTINGS, "打开设置", true, None::<&str>)?;
    let item_lock = MenuItem::with_id(app, ID_LOCK, "立即锁屏", true, None::<&str>)?;
    let item_unlock = MenuItem::with_id(app, ID_UNLOCK, "解锁", true, None::<&str>)?;
    let item_off = MenuItem::with_id(app, ID_SCREEN_OFF, "熄屏", true, None::<&str>)?;
    let item_on = MenuItem::with_id(app, ID_SCREEN_ON, "唤醒", true, None::<&str>)?;
    let item_quit = MenuItem::with_id(app, ID_QUIT, "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &item_settings,
            &item_lock,
            &item_unlock,
            &item_off,
            &item_on,
            &item_quit,
        ],
    )?;
    // ////////////////  核心逻辑：托盘  start  //////////////////////////////////
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("bundle icon 必须存在（tauri.conf.json bundle.icon）");
    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("remote-screen-lock 被控端")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app_handle, event| handle_menu(app_handle, event.id.0.as_str()))
        .build(app)?;
    Ok(())
    // ////////////////  核心逻辑  end  //////////////////////////////////////////
}

/// 托盘菜单事件处理。
/// 菜单回调运行在主线程：涉及窗口创建/对话框的动作转发到独立线程执行（规避 Webview2 主线程死锁）。
fn handle_menu(app: &AppHandle, id: &str) {
    let state: SharedState = app.state::<SharedState>().inner().clone();
    match id {
        ID_SETTINGS => open_settings(app),
        ID_LOCK => {
            let st = state.clone();
            std::thread::spawn(move || st.lock_now(None, "local(tray)"));
        }
        ID_UNLOCK => {
            let st = state.clone();
            std::thread::spawn(move || st.unlock_now("local(tray)"));
        }
        ID_SCREEN_OFF => {
            let st = state.clone();
            std::thread::spawn(move || {
                if let Err(e) = st.screen_off_now("local(tray)") {
                    st.log("local(tray)", "screen/off", &format!("err:{e}"));
                }
            });
        }
        ID_SCREEN_ON => {
            let st = state.clone();
            std::thread::spawn(move || {
                if let Err(e) = st.screen_on_now("local(tray)") {
                    st.log("local(tray)", "screen/on", &format!("err:{e}"));
                }
            });
        }
        ID_QUIT => confirm_and_exit(app, state),
        _ => {}
    }
}

/// 打开（或复用）设置窗口
fn open_settings(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        // 已存在：显示并聚焦
        let _ = win.show();
        let _ = win.set_focus();
    } else {
        // 重建（用户手动关闭后窗口被隐藏而非销毁，此分支兜底）
        let app = app.clone();
        std::thread::spawn(move || {
            let builder = tauri::WebviewWindowBuilder::new(
                &app,
                "settings",
                tauri::WebviewUrl::App("settings.html".into()),
            )
            .title("remote-screen-lock 设置")
            .inner_size(1200.0, 800.0)
            .min_inner_size(960.0, 640.0)
            .center()
            .resizable(true);
            if let Err(e) = builder.build() {
                eprintln!("[tray] 打开设置窗口失败: {e}");
            }
        });
    }
}

/// 退出二次确认（F-P0-01）：原生确认框；确认后退出进程
fn confirm_and_exit(app: &AppHandle, state: SharedState) {
    let app = app.clone();
    std::thread::spawn(move || {
        // blocking_show 不能在主线程调用（会冻结事件循环），此处在独立线程执行
        let yes = app
            .dialog()
            .message("退出后手机将无法远程控制本机，确认退出 remote-screen-lock 吗？")
            .title("退出确认")
            .buttons(MessageDialogButtons::OkCancelCustom("退出".into(), "取消".into()))
            .blocking_show();
        if yes {
            state.log("local(tray)", "app-quit", "ok");
            // 退出前释放防休状态（避免残留 ES_CONTINUOUS|SYSTEM_REQUIRED）
            platform::os::set_keep_awake(false);
            lock::close_lock_windows(&app);
            let _ = app.emit("rsl://app-quitting", ());
            app.exit(0);
        }
    });
}
