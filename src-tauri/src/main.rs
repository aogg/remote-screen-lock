//! remote-screen-lock 被端主入口（Tauri 2）。
//!
//! 启动顺序：
//! 1. 解析应用数据目录 → 加载/初始化配置（token 首次生成并持久化，F-P0-04）
//! 2. 构建 AppState 并 manage（IPC 命令可注入）
//! 3. 启动内置 HTTP 服务（axum，tauri::async_runtime 内运行，F-P0-02）
//! 4. 创建系统托盘（F-P0-01）+ 防休线程 + 锁定窗口监督任务（README-appendix 2）
//!
//! R-01：断电/重启后默认恢复 active（不自动锁定）；配置/token/图片全部保留。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // 发布版不弹控制台窗口

mod app_state;
mod commands;
mod config;
mod http_server;
mod lock;
mod net;
mod pairing;
mod platform;
mod tray;

use app_state::{AppState, Inner, SharedState};
use tauri::Manager;
use std::sync::{Arc, Mutex};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::regenerate_pairing,
            commands::reset_token,
            commands::save_settings,
            commands::change_local_key,
            commands::get_logs,
            commands::get_lock_state,
            commands::get_lock_image,
            commands::try_local_unlock,
            commands::hotkey_unlock,
        ])
        .setup(|app| {
            bootstrap(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // 全局窗口事件：拦截锁定窗口与设置窗的关闭请求
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                match window.label() {
                    "settings" => {
                        // 设置窗关闭 = 隐藏到托盘（应用继续常驻）
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    label if label == lock::LOCK_MAIN_LABEL || label.starts_with("lock-sub-") => {
                        // 锁定窗口不可被 Alt+F4 等方式关闭（S-08）
                        api.prevent_close();
                    }
                    _ => {}
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("remote-screen-lock 启动失败");
}

/// 应用初始化：目录/配置/状态/HTTP/托盘/监督任务
fn bootstrap(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // ////////////////  准备数据：目录与配置  start  ///////////////////////////
    let data_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."));
    std::fs::create_dir_all(&data_dir)?;
    let images_dir = data_dir.join("images");
    std::fs::create_dir_all(&images_dir)?;
    let config_path = data_dir.join("config.json");
    // F-P0-04：首次启动生成 token 后必须立即持久化（重启后 token 保持不变）
    let config_existed = config_path.exists();
    let cfg = config::load_or_init(&config_path);
    if !config_existed {
        if let Err(e) = config::save(&cfg, &config_path) {
            // 持久化失败仅告警（下次启动将重新生成 token，需重新配对）
            eprintln!("[boot] 初始配置写入失败: {e}");
        }
    }
    if !cfg.local_key_is_user_set {
        eprintln!("[boot] 首次启动：本地解锁密钥未设置，请在 设置→安全 立即设置");
    }
    // ////////////////  核心逻辑：全局状态  start  /////////////////////////////
    let state: SharedState = Arc::new(AppState {
        app: app.clone(),
        started_at: std::time::Instant::now(),
        inner: Mutex::new(Inner {
            actual_port: cfg.port, // 先以配置端口占位，HTTP 绑定成功后回写真实值
            config: cfg,
            config_path,
            images_dir,
            locked: false,      // R-01：重启后默认 active
            display_on: true,
            locked_mode: config::Mode::Clock,
            pairing: None,
            pairing_fails: 0,
            pairing_locked_until: None,
            auth_failures: std::collections::HashMap::new(),
            local_fails: 0,
            local_frozen_until: None,
            logs: std::collections::VecDeque::new(),
        }),
    });
    app.manage(state.clone());
    state.log("system", "boot", &format!("ok v{}", app_state::app_version()));
    // ////////////////  核心逻辑：常驻任务  start  /////////////////////////////
    // 1) HTTP 服务（含端口重试与崩溃重启）
    tauri::async_runtime::spawn(http_server::run(state.clone()));
    // 2) Windows 防休线程（SetThreadExecutionState 常驻持有者）
    platform::os::start_keep_awake_thread();
    // 3) 系统托盘
    tray::create(app)?;
    // 4) 锁定窗口监督（异常丢失 5 秒内重建）
    lock::start_supervisor(state.clone());
    Ok(())
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}
