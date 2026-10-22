//! Tauri IPC 命令（桌面设置窗 settings.html 与锁定窗口 lock.html 调用；withGlobalTauri 暴露 invoke）。

use crate::app_state::{self, SharedState};
use crate::config::Mode;
use crate::pairing;
use serde_json::{json, Value};
use tauri::{Emitter, State};

// ============================ 设置窗命令 ============================

/// 设置页首屏数据：配置（不含 token 全文）+ 配对信息（二维码/配对码/地址）
#[tauri::command]
pub fn get_settings(state: State<'_, SharedState>) -> Result<Value, String> {
    // ////////////////  准备数据  start  //////////////////////////////////////
    let (cfg_view, actual_port, token, configured_port) = {
        let inner = state.inner.lock().unwrap();
        let cfg = &inner.config;
        (
            json!({
                "port": cfg.port,
                "systemLockEnabled": cfg.system_lock_enabled,
                "defaultMode": cfg.default_mode.as_str(),
                "hour12": cfg.hour_12,
                "imageMaskPercent": cfg.image_mask_percent,
                "unlockHotkey": cfg.unlock_hotkey,
                "localKeyIsUserSet": cfg.local_key_is_user_set,
                "imageCount": cfg.images.len(),
            }),
            inner.actual_port,
            cfg.token.clone(),
            cfg.port,
        )
    };
    // ////////////////  核心逻辑：配对信息  start  /////////////////////////////
    let (code, expires_at) = state.ensure_pairing();
    let urls = crate::net::lan_base_urls(actual_port);
    // 二维码内容：http://<局域网IP>:<端口>/pair#t=<token>（仅本机界面展示）
    let qr_url = format!("{}/pair#t={token}", urls[0]);
    let qr_svg = pairing::qr_svg(&qr_url)?;
    let expires_unix = app_state::unix_now() + (expires_at - std::time::Instant::now()).as_secs().max(0);
    Ok(json!({
        "config": cfg_view,
        "pairing": {
            "code": code,
            "codeFormatted": pairing::format_pairing_code(&code),
            "expiresAtUnix": expires_unix,
            "qrSvg": qr_svg,
            "urls": urls,
            "tokenPreview": pairing::mask_token(&token),
        },
        "actualPort": actual_port,
        "portChanged": actual_port != configured_port,
        "deviceName": app_state::device_name(),
        "version": app_state::app_version(),
        "os": app_state::os_name(),
    }))
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 重新生成配对码
#[tauri::command]
pub fn regenerate_pairing(state: State<'_, SharedState>) -> Result<Value, String> {
    let (code, expires_at) = state.regenerate_pairing();
    let expires_unix = app_state::unix_now() + (expires_at - std::time::Instant::now()).as_secs().max(0);
    Ok(json!({
        "code": code,
        "codeFormatted": pairing::format_pairing_code(&code),
        "expiresAtUnix": expires_unix,
    }))
}

/// 重置 token（US-10：旧手机全部失效）
#[tauri::command]
pub fn reset_token(state: State<'_, SharedState>) -> Result<Value, String> {
    let new_token = state.reset_token();
    Ok(json!({ "ok": true, "tokenPreview": pairing::mask_token(&new_token) }))
}

/// 保存设置（F-P0-15：全部修改持久化；端口修改重启生效）
#[tauri::command]
pub fn save_settings(
    state: State<'_, SharedState>,
    payload: Value,
) -> Result<Value, String> {
    // ////////////////  校验参数  start  //////////////////////////////////////
    if let Some(port) = payload.get("port").and_then(|v| v.as_u64()) {
        if !(1024..=65535).contains(&port) {
            return Err("端口范围 1024~65535".into());
        }
    }
    if let Some(mask) = payload.get("imageMaskPercent").and_then(|v| v.as_u64()) {
        if mask > 60 {
            return Err("遮罩范围 0~60".into());
        }
    }
    if let Some(hotkey) = payload.get("unlockHotkey").and_then(|v| v.as_str()) {
        if !valid_hotkey(hotkey) {
            return Err("快捷键格式如 ctrl+alt+q（修饰键 ctrl/alt/shift + 单个字母或数字）".into());
        }
    }
    if let Some(mode) = payload.get("defaultMode").and_then(|v| v.as_str()) {
        if Mode::parse(mode).is_none() {
            return Err("模式仅支持 clock/image/black".into());
        }
        if mode == "image" && state.inner.lock().unwrap().config.images.is_empty() {
            return Err("尚未上传屏保图片，无法设为图片模式".into());
        }
    }
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    {
        let mut inner = state.inner.lock().unwrap();
        if let Some(port) = payload.get("port").and_then(|v| v.as_u64()) {
            inner.config.port = port as u16;
        }
        if let Some(v) = payload.get("systemLockEnabled").and_then(|v| v.as_bool()) {
            inner.config.system_lock_enabled = v;
        }
        if let Some(m) = payload.get("defaultMode").and_then(|v| v.as_str()).and_then(Mode::parse) {
            inner.config.default_mode = m;
            if inner.locked {
                inner.locked_mode = m;
            }
        }
        if let Some(v) = payload.get("hour12").and_then(|v| v.as_bool()) {
            inner.config.hour_12 = v;
        }
        if let Some(v) = payload.get("imageMaskPercent").and_then(|v| v.as_u64()) {
            inner.config.image_mask_percent = v as u8;
        }
        if let Some(v) = payload.get("unlockHotkey").and_then(|v| v.as_str()) {
            inner.config.unlock_hotkey = v.to_string();
        }
    }
    state.save_config();
    // 锁定中改模式/制式 → 通知锁定窗口重渲染
    let locked = state.inner.lock().unwrap().locked;
    if locked {
        let img = state.active_image_path();
        let mode = state.inner.lock().unwrap().locked_mode;
        let _ = state.app.emit_to(
            crate::lock::LOCK_MAIN_LABEL,
            "rsl://mode-changed",
            json!({ "mode": mode.as_str(), "imagePath": img }),
        );
    }
    Ok(json!({ "ok": true }))
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 校验快捷键格式：小写修饰键(ctrl/alt/shift，可组合) + 单个 a-z0-9
fn valid_hotkey(s: &str) -> bool {
    let parts: Vec<&str> = s.split('+').collect();
    if parts.len() < 2 {
        return false;
    }
    let (mods, key) = parts.split_at(parts.len() - 1);
    let key = key[0];
    if key.len() != 1 || !(key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())) {
        return false;
    }
    !mods.is_empty()
        && mods.iter().all(|m| matches!(*m, "ctrl" | "alt" | "shift"))
        // 修饰键不重复
        && mods.len() == mods.iter().collect::<std::collections::HashSet<_>>().len()
}

/// 修改本地解锁密钥（F-P0-09：4~6 位数字；hash 持久化，明文即焚）
#[tauri::command]
pub fn change_local_key(
    state: State<'_, SharedState>,
    old_key: Option<String>,
    new_key: String,
) -> Result<Value, String> {
    // ////////////////  校验参数  start  //////////////////////////////////////
    let nk = new_key.trim().to_string();
    if !(4..=6).contains(&nk.len()) || !nk.chars().all(|c| c.is_ascii_digit()) {
        return Err("密钥须为 4~6 位数字".into());
    }
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    {
        let mut inner = state.inner.lock().unwrap();
        if inner.config.local_key_is_user_set {
            // 已设置过：需验证旧密钥
            let old = old_key.unwrap_or_default();
            if !crate::config::verify_local_key(&old, &inner.config.local_key_hash) {
                return Err("旧密钥不正确".into());
            }
        }
        inner.config.local_key_hash = crate::config::hash_local_key(&nk);
        inner.config.local_key_is_user_set = true;
    }
    state.save_config();
    state.log("local", "key-change", "ok");
    Ok(json!({ "ok": true }))
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 读取操作日志（最近 200 条）
#[tauri::command]
pub fn get_logs(state: State<'_, SharedState>) -> Value {
    let inner = state.inner.lock().unwrap();
    let logs: Vec<Value> = inner
        .logs
        .iter()
        .map(|l| {
            json!({
                "ts": l.ts,
                "source": l.source,
                "action": l.action,
                "result": l.result,
            })
        })
        .collect();
    json!({ "logs": logs })
}

// ============================ 锁定窗口命令 ============================

/// 锁定页首屏数据（lock.html invoke）
#[tauri::command]
pub fn get_lock_state(state: State<'_, SharedState>) -> Result<Value, String> {
    let inner = state.inner.lock().unwrap();
    Ok(json!({
        "mode": inner.locked_mode.as_str(),
        "hour12": inner.config.hour_12,
        "imageMaskPercent": inner.config.image_mask_percent,
        "hotkey": inner.config.unlock_hotkey,
        "hasKey": true,
    }))
}

/// 锁定页取当前背景图绝对路径（asset 协议转换在前端做）
#[tauri::command]
pub fn get_lock_image(state: State<'_, SharedState>) -> Value {
    json!({ "path": state.active_image_path().map(|p| p.to_string_lossy().into_owned()) })
}

/// 密钥解锁（F-P0-09：argon2 校验 + 5 次冻结 30 秒）
#[tauri::command]
pub async fn try_local_unlock(
    state: State<'_, SharedState>,
    key: String,
) -> Result<Value, String> {
    let r = state.try_local_unlock(&key);
    Ok(json!({
        "ok": r.ok,
        "frozenRemainingSec": r.frozen_remaining_sec,
        "remainingAttempts": r.remaining_attempts,
    }))
}

/// 快捷键直接解锁（默认 Ctrl+Alt+Q；坐在电脑前的人的快速通道，US-09）
#[tauri::command]
pub async fn hotkey_unlock(state: State<'_, SharedState>) -> Result<Value, String> {
    state.unlock_now("local(hotkey)");
    Ok(json!({ "ok": true }))
}
