//! 全局应用状态：状态机（active/locked/screen_off）+ 配置 + 配对 + 限速 + 日志。
//!
//! 状态机（interaction-ui.md 5.3 实现基准）：
//! ```text
//!   active ──lock──► locked ──screen/off──► screen_off（保留 lockedMode）
//!   locked ──unlock──► active
//!   screen_off ──wake──► locked 或 active（恢复熄屏前状态）
//!   locked/screen_off 下 PUT /mode 即时切换子模式（clock/image/black）
//! ```
//! 所有指令幂等：重复 lock 返回当前状态。

use crate::config::{Config, ImageMeta, Mode};
use crate::pairing;
use crate::{lock, platform};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// 全局共享状态类型（注入 axum Router 与 Tauri Manager）
pub type SharedState = Arc<AppState>;

/// 配对码会话状态（内存态，不持久化；10 分钟有效、一次性）
pub struct PairingState {
    /// 8 位配对码
    pub code: String,
    /// 过期时刻
    pub expires_at: Instant,
}

/// 操作日志条目（F-P0-15：最近 200 条，token 脱敏在写入前完成）
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    /// 时间（Unix 秒）
    pub ts: u64,
    /// 来源：remote(<ip>) / local / system
    pub source: String,
    /// 动作：POST /lock、key-unlock、pair 等
    pub action: String,
    /// 结果：ok / err:<原因>
    pub result: String,
}

/// 可变内部状态（单一互斥锁保护，锁内无 await）
pub struct Inner {
    /// 持久化配置（token/端口/模式/密钥哈希/图片列表）
    pub config: Config,
    /// 配置文件路径
    pub config_path: PathBuf,
    /// 图片存储目录（app_data/images）
    pub images_dir: PathBuf,
    /// 应用锁是否生效
    pub locked: bool,
    /// 显示器是否点亮
    pub display_on: bool,
    /// 当前锁定窗口渲染模式（locked 时有意义）
    pub locked_mode: Mode,
    /// HTTP 实际监听端口（端口被占用时可能与配置不同并在设置窗提示）
    pub actual_port: u16,
    /// 当前配对码会话
    pub pairing: Option<PairingState>,
    /// 配对码连续错误次数（5 次锁 5 分钟，S-02）
    pub pairing_fails: u32,
    /// 配对码锁定截止时刻
    pub pairing_locked_until: Option<Instant>,
    /// 鉴权失败限速：同 IP 滑动窗口（10 次/分钟，S-02）
    pub auth_failures: HashMap<IpAddr, VecDeque<Instant>>,
    /// 本地密钥连续错误次数（5 次冻结 30 秒，F-P0-09）
    pub local_fails: u32,
    /// 本地密钥输入冻结截止时刻
    pub local_frozen_until: Option<Instant>,
    /// 环形日志（最多 200 条）
    pub logs: VecDeque<LogEntry>,
}

/// 应用全局状态
pub struct AppState {
    /// Tauri 应用句柄（窗口/事件操作）
    pub app: AppHandle,
    /// 启动时刻（uptimeSec 统计）
    pub started_at: Instant,
    /// 内部可变状态
    pub inner: Mutex<Inner>,
}

/// 当前 Unix 秒
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 主机名（状态接口 deviceName；获取失败回退 "unknown-host"）
pub fn device_name() -> String {
    hostname::get()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown-host".into())
}

/// 操作系统标识（status.os：windows / macos / linux）
pub fn os_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        "linux"
    }
}

/// 应用版本（与 tauri.conf.json version 一致）
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

impl AppState {
    /// 写一条操作日志（环形 200 条）
    pub fn log(&self, source: &str, action: &str, result: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.logs.push_back(LogEntry {
            ts: unix_now(),
            source: source.to_string(),
            action: action.to_string(),
            result: result.to_string(),
        });
        while inner.logs.len() > 200 {
            inner.logs.pop_front();
        }
        // 同步输出到 stderr，方便 CI/调试排查
        eprintln!("[log] {} {} {} {}", unix_now(), source, action, result);
    }

    /// 持久化配置并广播配置变更事件（设置窗/锁屏窗监听刷新）
    pub fn save_config(&self) {
        let (cfg, path) = {
            let inner = self.inner.lock().unwrap();
            (inner.config.clone(), inner.config_path.clone())
        };
        if let Err(e) = crate::config::save(&cfg, &path) {
            self.log("system", "config-save", &format!("err:{e}"));
        }
        // 广播（无监听者时忽略）
        let _ = self.app.emit("rsl://config-changed", ());
    }

    // ============================ 配对 ============================

    /// 确保存在有效配对码（无/过期则生成，10 分钟有效）
    pub fn ensure_pairing(&self) -> (String, Instant) {
        let mut inner = self.inner.lock().unwrap();
        let need_new = inner
            .pairing
            .as_ref()
            .map(|p| p.expires_at <= Instant::now())
            .unwrap_or(true);
        if need_new {
            inner.pairing = Some(PairingState {
                code: pairing::generate_pairing_code(),
                expires_at: Instant::now() + Duration::from_secs(pairing::PAIRING_TTL_SECS),
            });
        }
        let p = inner.pairing.as_ref().unwrap();
        (p.code.clone(), p.expires_at)
    }

    /// 重新生成配对码（设置窗按钮）
    pub fn regenerate_pairing(&self) -> (String, Instant) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.pairing = Some(PairingState {
                code: pairing::generate_pairing_code(),
                expires_at: Instant::now() + Duration::from_secs(pairing::PAIRING_TTL_SECS),
            });
            // 重新生成同时清除锁定与错误计数（给用户一条出路）
            inner.pairing_locked_until = None;
            inner.pairing_fails = 0;
        }
        self.log("local", "pair-regen", "ok");
        self.ensure_pairing()
    }

    /// 重置 token（US-10：旧 token 立即失效，旧手机收到 401 进入重新配对引导）
    pub fn reset_token(&self) -> String {
        let new_token;
        {
            let mut inner = self.inner.lock().unwrap();
            new_token = pairing::generate_token();
            inner.config.token = new_token.clone();
        }
        self.save_config();
        // token 重置后旧配对码一并作废，避免旧码换新 token
        self.regenerate_pairing();
        self.log("local", "token-reset", "ok");
        new_token
    }

    /// 配对码换 token 的错误类型
    pub enum PairError {
        /// 尝试次数过多（携带剩余等待秒数）
        Locked(u64),
        /// 配对码错误（携带剩余尝试次数）
        Wrong(u32),
    }

    /// 校验配对码并发放 token（POST /api/v1/pair）
    pub fn try_pair(&self, input: &str) -> Result<String, PairError> {
        // ////////////////  校验参数  start  //////////////////////////////////////
        let code = pairing::normalize_pairing_code(input);
        // 长度不合法也走下方统一“配对码错误”计数（存储码恒为 8 位，必不匹配）
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        let mut inner = self.inner.lock().unwrap();
        // 1) 锁定期检查（5 次错误锁 5 分钟，S-02）
        if let Some(until) = inner.pairing_locked_until {
            if until > Instant::now() {
                return Err(PairError::Locked((until - Instant::now()).as_secs().max(1)));
            }
            inner.pairing_locked_until = None;
        }
        // 2) 配对码有效性：存在、未过期、且匹配
        let ok = inner
            .pairing
            .as_ref()
            .map(|p| p.expires_at > Instant::now() && p.code == code)
            .unwrap_or(false);
        if !ok {
            inner.pairing_fails += 1;
            if inner.pairing_fails >= 5 {
                inner.pairing_locked_until = Some(Instant::now() + Duration::from_secs(300));
                inner.pairing_fails = 0;
                return Err(PairError::Locked(300));
            }
            return Err(PairError::Wrong(5 - inner.pairing_fails));
        }
        // 3) 成功：配对码一次性作废（S-03），发放当前 token
        inner.pairing = None;
        inner.pairing_fails = 0;
        Ok(inner.config.token.clone())
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    // ============================ 鉴权与限速 ============================

    /// 校验 Bearer token 是否有效
    pub fn token_valid(&self, token: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        // 注：局域网场景下简单比对（S-01 已由 32 位随机性保障）
        !token.is_empty() && inner.config.token == token
    }

    /// 记录一次鉴权失败；返回 (是否超限, 建议重试等待秒数)
    pub fn record_auth_failure(&self, ip: IpAddr) -> (bool, u64) {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        let win = inner.auth_failures.entry(ip).or_default();
        win.push_back(now);
        // 只保留最近 60 秒
        while let Some(front) = win.front() {
            if now.duration_since(*front) > Duration::from_secs(60) {
                win.pop_front();
            } else {
                break;
            }
        }
        if win.len() >= 10 {
            // 附录 3：触发限速需在日志中告警标记
            drop(inner);
            self.log(&format!("remote({ip})"), "auth", "err:rate-limited(429)");
            return (true, 60);
        }
        (false, 0)
    }

    // ============================ 状态机动作 ============================

    /// 当前使用中的图片（image 模式背景；无则 None）
    pub fn active_image(&self) -> Option<ImageMeta> {
        let inner = self.inner.lock().unwrap();
        inner.config.images.iter().find(|m| m.in_use).cloned()
    }

    /// 当前使用中图片的绝对路径（锁定窗口经 asset 协议加载）
    pub fn active_image_path(&self) -> Option<std::path::PathBuf> {
        let inner = self.inner.lock().unwrap();
        inner
            .config
            .images
            .iter()
            .find(|m| m.in_use)
            .map(|m| inner.images_dir.join(format!("{}.{}", m.id, m.stored_ext)))
    }

    /// 立即锁屏（F-P0-06）。幂等：已锁定时仅可顺带切模式。
    /// source 例：`remote(192.168.1.31)` / `local(tray)` / `supervisor`
    pub fn lock_now(&self, mode: Option<Mode>, source: &str) {
        // ////////////////  准备数据  start  //////////////////////////////////////
        let (sys_lock, target_mode, image_path) = {
            let mut inner = self.inner.lock().unwrap();
            let target = mode.unwrap_or(inner.config.default_mode);
            if inner.locked {
                // 幂等：仅当指定了新模式时即时切换
                if mode.is_some() && inner.locked_mode != target {
                    inner.locked_mode = target;
                }
                return;
            }
            inner.locked = true;
            inner.locked_mode = target;
            let img = inner
                .config
                .images
                .iter()
                .find(|m| m.in_use)
                .map(|m| inner.images_dir.join(format!("{}.{}", m.id, m.stored_ext)));
            (inner.config.system_lock_enabled, target, img)
        };
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        // 1) 打开全屏置顶锁定窗口（主屏内容、副屏纯黑）
        if let Err(e) = lock::open_lock_windows(&self.app, target_mode, image_path.as_deref()) {
            self.log(source, "lock", &format!("err:window({e})"));
        }
        // 2) 系统锁（开关默认开）；失败不回滚窗口锁，仅记录（系统级失败可读日志排查）
        if sys_lock {
            match platform::os::system_lock() {
                Ok(()) => self.log(source, "lock", "ok(+system-lock)"),
                Err(e) => self.log(source, "lock", &format!("ok(window) err:system-lock({e})")),
            }
        } else {
            self.log(source, "lock", "ok");
        }
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    /// 解除应用锁（F-P0-10）：关闭锁定窗口 + 点亮显示器；幂等
    pub fn unlock_now(&self, source: &str) {
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        lock::close_lock_windows(&self.app);
        if let Err(e) = platform::os::wake_display() {
            self.log(source, "unlock", &format!("err:wake({e})"));
        }
        platform::os::set_keep_awake(false);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.locked = false;
            inner.display_on = true;
        }
        self.log(source, "unlock", "ok");
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    /// 熄屏（F-P0-11）：显示器关闭 + 阻止系统休眠保持 HTTP 可达；保留锁定状态
    pub fn screen_off_now(&self, source: &str) -> Result<(), String> {
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        platform::os::screen_off()?;
        platform::os::set_keep_awake(true); // 熄屏期间防休
        {
            let mut inner = self.inner.lock().unwrap();
            inner.display_on = false;
        }
        self.log(source, "screen/off", "ok");
        Ok(())
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    /// 唤醒显示器（F-P0-11）：点亮并恢复熄屏前状态（locked 仍锁定）
    pub fn screen_on_now(&self, source: &str) -> Result<(), String> {
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        platform::os::wake_display()?;
        platform::os::set_keep_awake(false);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.display_on = true;
        }
        self.log(source, "screen/on", "ok");
        Ok(())
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    /// 切换屏保模式（F-P0-07 / US-05）；未锁定仅记录默认模式
    pub fn set_mode(&self, mode: Mode, source: &str) -> Result<(), crate::http_server::api::ApiError> {
        // ////////////////  校验参数  start  //////////////////////////////////////
        if mode == Mode::Image && self.inner.lock().unwrap().config.images.is_empty() {
            // US-06：图片列表为空拒绝切换（40902）
            return Err(crate::http_server::api::ApiError::NoImages);
        }
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        let locked;
        {
            let mut inner = self.inner.lock().unwrap();
            inner.config.default_mode = mode;
            locked = inner.locked;
            if locked {
                inner.locked_mode = mode;
            }
        }
        self.save_config();
        // 锁定中：通知锁定窗口即时重渲染（无需解锁）
        if locked {
            let img = self.active_image_path();
            let _ = self.app.emit_to(
                "lock-main",
                "rsl://mode-changed",
                serde_json::json!({ "mode": mode.as_str(), "imagePath": img }),
            );
        }
        self.log(source, "PUT /mode", &format!("ok({})", mode.as_str()));
        Ok(())
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    // ============================ 本地解锁 ============================

    /// 本地密钥解锁结果
    pub struct LocalUnlockResult {
        /// 是否解锁成功
        pub ok: bool,
        /// 冻结剩余秒数（>0 表示处于冻结期）
        pub frozen_remaining_sec: u64,
        /// 剩余尝试次数提示（失败时）
        pub remaining_attempts: u32,
    }

    /// 校验本地解锁密钥（F-P0-09：argon2 校验 + 5 次错误冻结 30 秒）
    pub fn try_local_unlock(&self, key: &str) -> LocalUnlockResult {
        // ////////////////  校验参数（冻结期）  start  ////////////////////////////
        {
            let inner = self.inner.lock().unwrap();
            if let Some(until) = inner.local_frozen_until {
                if until > Instant::now() {
                    return LocalUnlockResult {
                        ok: false,
                        frozen_remaining_sec: (until - Instant::now()).as_secs().max(1),
                        remaining_attempts: 0,
                    };
                }
            }
        }
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        let stored = self.inner.lock().unwrap().config.local_key_hash.clone();
        // 密钥未设置：直接失败且不计错误次数（前端引导先去设置密钥；快捷键仍可解锁）
        if stored.is_empty() {
            self.log("local", "key-unlock", "err:no-key-set");
            return LocalUnlockResult { ok: false, frozen_remaining_sec: 0, remaining_attempts: 0 };
        }
        let ok = crate::config::verify_local_key(key, &stored);
        if ok {
            self.inner.lock().unwrap().local_fails = 0;
            self.unlock_now("local");
            self.log("local", "key-unlock", "ok");
            LocalUnlockResult { ok: true, frozen_remaining_sec: 0, remaining_attempts: 0 }
        } else {
            let mut inner = self.inner.lock().unwrap();
            inner.local_fails += 1;
            if inner.local_fails >= 5 {
                inner.local_frozen_until = Some(Instant::now() + Duration::from_secs(30));
                inner.local_fails = 0;
                drop(inner);
                self.log("local", "key-unlock", "err:frozen(30s)");
                LocalUnlockResult { ok: false, frozen_remaining_sec: 30, remaining_attempts: 0 }
            } else {
                let rem = 5 - inner.local_fails;
                drop(inner);
                self.log("local", "key-unlock", "err:wrong-key");
                LocalUnlockResult { ok: false, frozen_remaining_sec: 0, remaining_attempts: rem }
            }
        }
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    // ============================ 图片存储 ============================

    /// 图片入库错误（映射 S-06 错误码）
    pub enum ImageStoreError {
        /// 单文件超 10MB（41301）
        TooLarge,
        /// 格式不支持 / 无法解码（41501）
        BadFormat(String),
        /// 总量超 50MB（41301）
        TotalExceeded,
        /// 写盘失败（50001）
        Io(String),
    }

    /// 保存上传图片：魔数校验 + 解码取尺寸 + 服务端重命名存储（F-P0-12 / S-06）
    pub fn store_image(&self, file_name: &str, bytes: Vec<u8>) -> Result<ImageMeta, ImageStoreError> {
        // ////////////////  校验参数  start  //////////////////////////////////////
        const MAX_FILE: u64 = 10 * 1024 * 1024; // 单文件 10MB
        const MAX_TOTAL: u64 = 50 * 1024 * 1024; // 总量 50MB
        if bytes.len() as u64 > MAX_FILE {
            return Err(ImageStoreError::TooLarge);
        }
        if bytes.len() < 12 {
            return Err(ImageStoreError::BadFormat("文件过小".into()));
        }
        // 魔数校验：JPEG FF D8 FF / PNG 89 50 4E 47 / WEBP RIFF....WEBP
        let ext = if bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
            "jpg"
        } else if bytes[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
            "png"
        } else if bytes[0..4] == *b"RIFF" && bytes[8..12] == *b"WEBP" {
            "webp"
        } else {
            return Err(ImageStoreError::BadFormat("仅支持 jpg/png/webp".into()));
        };
        // 解码校验 + 提取尺寸（解码失败视为损坏文件 → 41501）
        let (w, h) = image::load_from_memory(&bytes)
            .map_err(|e| ImageStoreError::BadFormat(format!("解码失败: {e}")))
            .map(|img| (img.width(), img.height()))?;
        // ////////////////  核心逻辑  start  //////////////////////////////////////
        let mut inner = self.inner.lock().unwrap();
        let total: u64 = inner.config.images.iter().map(|m| m.size_bytes).sum();
        if total + bytes.len() as u64 > MAX_TOTAL {
            return Err(ImageStoreError::TotalExceeded);
        }
        let id = pairing::generate_image_id();
        let path = inner.images_dir.join(format!("{id}.{ext}"));
        std::fs::write(&path, &bytes).map_err(|e| ImageStoreError::Io(e.to_string()))?;
        // 首张/新上传图片自动设为使用中
        for m in inner.config.images.iter_mut() {
            m.in_use = false;
        }
        let meta = ImageMeta {
            id: id.clone(),
            file_name: file_name.to_string(),
            stored_ext: ext.to_string(),
            width: w,
            height: h,
            size_bytes: bytes.len() as u64,
            in_use: true,
            added_at: unix_now(),
        };
        inner.config.images.push(meta.clone());
        drop(inner);
        self.save_config();
        self.log("remote", "POST /images", &format!("ok({id})"));
        Ok(meta)
        // ////////////////  核心逻辑  end  ////////////////////////////////////////
    }

    /// 读取图片原始字节（GET /images/{id}/raw 缩略图/锁定页背景共用）
    pub fn image_raw(&self, id: &str) -> Option<(Vec<u8>, &'static str)> {
        let inner = self.inner.lock().unwrap();
        let meta = inner.config.images.iter().find(|m| m.id == id)?;
        let mime = match meta.stored_ext.as_str() {
            "jpg" => "image/jpeg",
            "png" => "image/png",
            "webp" => "image/webp",
            _ => "application/octet-stream",
        };
        let bytes = std::fs::read(inner.images_dir.join(format!("{id}.{}", meta.stored_ext))).ok()?;
        Some((bytes, mime))
    }

    /// 删除图片；删除"使用中"的图自动回退 clock 模式（F-P0-12 / P1-03 行为前置）
    pub fn delete_image(&self, id: &str, source: &str) -> Result<bool, String> {
        let mut fallback_mode = false;
        {
            let mut inner = self.inner.lock().unwrap();
            let before = inner.config.images.len();
            if let Some(pos) = inner.config.images.iter().position(|m| m.id == id) {
                let meta = inner.config.images.remove(pos);
                let _ = std::fs::remove_file(inner.images_dir.join(format!("{id}.{}", meta.stored_ext)));
                if meta.in_use {
                    // 使用中的图被删：回退 clock（若当前正以 image 锁定则一并切换）
                    if inner.config.default_mode == Mode::Image {
                        inner.config.default_mode = Mode::Clock;
                        fallback_mode = true;
                    }
                    if inner.locked && inner.locked_mode == Mode::Image {
                        inner.locked_mode = Mode::Clock;
                    }
                }
            }
            if inner.config.images.len() == before {
                return Ok(false); // 不存在 → 幂等返回
            }
        }
        self.save_config();
        if fallback_mode {
            let _ = self.app.emit_to(
                "lock-main",
                "rsl://mode-changed",
                serde_json::json!({ "mode": "clock", "imagePath": null }),
            );
        }
        self.log(source, "DELETE /images", &format!("ok({id})"));
        Ok(true)
    }

    // ============================ 状态快照 ============================

    /// GET /api/v1/status 数据（F-P0-13）
    pub fn status_snapshot(&self) -> serde_json::Value {
        let inner = self.inner.lock().unwrap();
        // state 推导：熄屏优先（锁定+熄屏=screen_off，附 lockedMode），其次 locked/active
        let state = if !inner.display_on {
            "screen_off"
        } else if inner.locked {
            "locked"
        } else {
            "active"
        };
        serde_json::json!({
            "deviceName": device_name(),
            "os": os_name(),
            "version": app_version(),
            "state": state,
            "lockedMode": inner.locked_mode.as_str(),
            "defaultMode": inner.config.default_mode.as_str(),
            "displayOn": inner.display_on,
            "brightness": serde_json::Value::Null, // P1（F-P1-02）；null 时控制台隐藏滑杆
            "systemLockEnabled": inner.config.system_lock_enabled,
            "localTime": unix_now(),
            "uptimeSec": self.started_at.elapsed().as_secs(),
        })
    }
}
