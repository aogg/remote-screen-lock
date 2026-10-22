//! REST API 处理器（interaction-ui.md 4.3 路由总表 + 4.5 错误码 + 4.1 响应包络）。

use crate::app_state::{self, SharedState};
use crate::config::Mode;
use axum::extract::{ConnectInfo, Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use tauri::Emitter;
use serde_json::json;
use std::net::SocketAddr;

// ============================ 错误码（4.5） ============================

/// API 错误：实现 IntoResponse，按 interaction-ui.md 4.5 输出包络
#[derive(Debug)]
pub enum ApiError {
    /// 40001 参数缺失或格式错误
    InvalidParam,
    /// 40101 缺少 token
    MissingToken,
    /// 40102 token 无效
    InvalidToken,
    /// 40103 配对码错误（携带剩余次数）
    PairCodeWrong { remaining: u32 },
    /// 40901 状态冲突
    Conflict,
    /// 40902 无屏保图片却切 image 模式
    NoImages,
    /// 41301 图片超限（单文件 10MB / 总量 50MB，携带具体文案）
    TooLarge(&'static str),
    /// 41501 图片格式不支持
    BadFormat,
    /// 42901 尝试次数过多（携带剩余等待秒数）
    TooManyAttempts { retry_after_sec: u64 },
    /// 50001 内部错误
    Internal(String),
    /// 50301 系统命令执行失败
    SystemCmdFailed(String),
    /// 50302 设备不支持该操作
    NotSupported,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // ////////////////  核心逻辑：错误码→HTTP/文案  start  ///////////////////
        let (status, code, msg, data) = match self {
            ApiError::InvalidParam => (StatusCode::BAD_REQUEST, 40001u32, "请求参数有误".into(), json!(null)),
            ApiError::MissingToken => (StatusCode::UNAUTHORIZED, 40101, "未配对，请重新配对".into(), json!(null)),
            ApiError::InvalidToken => (StatusCode::UNAUTHORIZED, 40102, "token 无效".into(), json!(null)),
            ApiError::PairCodeWrong { remaining } => (
                StatusCode::UNAUTHORIZED,
                40103,
                format!("配对码不正确（剩余 {remaining} 次机会）"),
                json!(null),
            ),
            ApiError::Conflict => (StatusCode::CONFLICT, 40901, "当前状态不支持该操作".into(), json!(null)),
            ApiError::NoImages => (
                StatusCode::CONFLICT,
                40902,
                "尚未上传屏保图片，无法切换到图片模式".into(),
                json!(null),
            ),
            ApiError::TooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, 41301, (*msg).into(), json!(null)),
            ApiError::BadFormat => (StatusCode::UNSUPPORTED_MEDIA_TYPE, 41501, "仅支持 jpg/png/webp".into(), json!(null)),
            ApiError::TooManyAttempts { retry_after_sec } => (
                StatusCode::TOO_MANY_REQUESTS,
                42901,
                format!("尝试次数过多，请 {retry_after_sec} 秒后重试"),
                json!({ "retryAfterSec": retry_after_sec }),
            ),
            ApiError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, 50001, format!("被控端内部错误: {e}"), json!(null)),
            ApiError::SystemCmdFailed(e) => (
                StatusCode::SERVICE_UNAVAILABLE,
                50301,
                format!("系统操作被拒绝，请检查系统设置: {e}"),
                json!(null),
            ),
            ApiError::NotSupported => (
                StatusCode::SERVICE_UNAVAILABLE,
                50302,
                "当前显示器不支持软件调光".into(),
                json!(null),
            ),
        };
        (status, Json(json!({ "code": code, "msg": msg, "data": data }))).into_response()
        // ////////////////  核心逻辑  end  //////////////////////////////////////
    }
}

/// 成功包络：{"code":0,"msg":"ok","data":...}
fn ok<T: serde::Serialize>(data: T) -> Response {
    Json(json!({ "code": 0, "msg": "ok", "data": data })).into_response()
}

/// 来源标记（操作日志用）：remote(<ip>)
fn src_of(addr: &SocketAddr) -> String {
    format!("remote({})", addr.ip())
}

// ============================ 公开接口 ============================

/// GET /healthz：健康检查，仅返回版本（S-07：不暴露主机名等敏感信息，不走包络）
pub async fn healthz() -> Response {
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        json!({ "version": app_state::app_version() }).to_string(),
    )
        .into_response()
}

/// POST /api/v1/pair 请求体
#[derive(Deserialize)]
pub struct PairReq {
    /// 8 位配对码
    pub code: String,
    /// 设备名（手机自报，仅日志记录用；可缺省）
    #[serde(default)]
    pub device: String,
}

/// POST /api/v1/pair：配对码换 token（F-P0-04）
pub async fn pair_submit(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<PairReq>,
) -> Response {
    let _ = req.device; // 仅日志可选用，P0 不记录设备名（避免多余信息收集）
    match state.try_pair(&req.code) {
        Ok(token) => {
            state.log(&src_of(&addr), "POST /pair", "ok");
            ok(json!({
                "token": token,
                "deviceName": app_state::device_name(),
                "version": app_state::app_version(),
            }))
        }
        Err(app_state::PairError::Locked(secs)) => {
            state.log(&src_of(&addr), "POST /pair", "err:locked");
            ApiError::TooManyAttempts { retry_after_sec: secs }.into_response()
        }
        Err(app_state::PairError::Wrong(remaining)) => {
            state.log(&src_of(&addr), "POST /pair", "err:wrong-code");
            ApiError::PairCodeWrong { remaining }.into_response()
        }
    }
}

// ============================ 受保护接口 ============================

/// POST /api/v1/pair/verify：校验 token（扫码进入时静默校验）
pub async fn pair_verify() -> Response {
    ok(json!({
        "deviceName": app_state::device_name(),
        "version": app_state::app_version(),
    }))
}

/// GET /api/v1/status：状态总览（F-P0-13）
pub async fn status(State(state): State<SharedState>) -> Response {
    ok(state.status_snapshot())
}

/// POST /api/v1/lock 可选请求体
#[derive(Deserialize, Default)]
pub struct LockReq {
    /// 锁定同时切换模式（可缺省）
    pub mode: Option<String>,
}

/// 解析可选模式参数（非法 → 40001）
fn parse_mode(opt: &Option<String>) -> Result<Option<Mode>, ApiError> {
    match opt {
        None => Ok(None),
        Some(s) => Mode::parse(s).map(Some).ok_or(ApiError::InvalidParam),
    }
}

/// POST /api/v1/lock：立即锁屏（F-P0-06；幂等，US-02）
pub async fn lock(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Option<Json<LockReq>>,
) -> Result<Response, ApiError> {
    let mode = parse_mode(&body.map(|Json(b)| b.mode).unwrap_or_default())?;
    // image 模式需已有图片
    if mode == Some(Mode::Image) && state.inner.lock().unwrap().config.images.is_empty() {
        return Err(ApiError::NoImages);
    }
    state.lock_now(mode, &src_of(&addr));
    // 锁定后读取最新状态返回（含 lockedMode）
    let snap = state.status_snapshot();
    Ok(ok(json!({ "state": snap["state"], "lockedMode": snap["lockedMode"] })))
}

/// POST /api/v1/unlock：解除应用锁（F-P0-10；幂等，US-03）
pub async fn unlock(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    state.unlock_now(&src_of(&addr));
    ok(json!({ "state": "active" }))
}

/// POST /api/v1/screen/off：熄屏（F-P0-11；保留锁定状态）
pub async fn screen_off(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Response, ApiError> {
    // 幂等：已熄屏直接返回当前状态
    if !state.inner.lock().unwrap().display_on {
        let snap = state.status_snapshot();
        return Ok(ok(json!({ "state": snap["state"], "lockedMode": snap["lockedMode"] })));
    }
    state
        .screen_off_now(&src_of(&addr))
        .map_err(|e| ApiError::SystemCmdFailed(e))?;
    let snap = state.status_snapshot();
    Ok(ok(json!({ "state": snap["state"], "lockedMode": snap["lockedMode"] })))
}

/// POST /api/v1/screen/on：唤醒显示器（F-P0-11；恢复熄屏前状态）
pub async fn screen_on(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<Response, ApiError> {
    if state.inner.lock().unwrap().display_on {
        let snap = state.status_snapshot();
        return Ok(ok(json!({ "state": snap["state"], "lockedMode": snap["lockedMode"] })));
    }
    state
        .screen_on_now(&src_of(&addr))
        .map_err(|e| ApiError::SystemCmdFailed(e))?;
    let snap = state.status_snapshot();
    Ok(ok(json!({ "state": snap["state"], "lockedMode": snap["lockedMode"] })))
}

/// GET /api/v1/mode：查询当前/默认模式
pub async fn get_mode(State(state): State<SharedState>) -> Response {
    let inner = state.inner.lock().unwrap();
    ok(json!({
        "mode": inner.config.default_mode.as_str(),
        "lockedMode": inner.locked_mode.as_str(),
        "locked": inner.locked,
    }))
}

/// PUT /api/v1/mode 请求体
#[derive(Deserialize)]
pub struct ModeReq {
    /// 目标模式 clock/image/black
    pub mode: String,
}

/// PUT /api/v1/mode：设置屏保模式（F-P0-07 / US-05）
pub async fn set_mode(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<ModeReq>,
) -> Result<Response, ApiError> {
    let mode = Mode::parse(&req.mode).ok_or(ApiError::InvalidParam)?;
    state.set_mode(mode, &src_of(&addr))?;
    Ok(ok(json!({ "mode": mode.as_str() })))
}

/// POST /api/v1/images：上传屏保图片（multipart 字段 file，F-P0-12 / S-06）
pub async fn upload_image(
    State(state): State<SharedState>,
    mut mp: Multipart,
) -> Result<Response, ApiError> {
    // ////////////////  校验参数：取 file 字段  start  /////////////////////////
    let mut got: Option<(String, Vec<u8>)> = None;
    while let Some(field) = mp
        .next_field()
        .await
        .map_err(|_| ApiError::InvalidParam)?
    {
        if field.name() != Some("file") {
            continue; // 忽略其他字段
        }
        let file_name = field.file_name().unwrap_or("upload.bin").to_string();
        let bytes = field.bytes().await.map_err(|_| ApiError::InvalidParam)?;
        got = Some((file_name, bytes.to_vec()));
        break;
    }
    let (file_name, bytes) = got.ok_or(ApiError::InvalidParam)?;
    // ////////////////  核心逻辑：入库  start  /////////////////////////////////
    match state.store_image(&file_name, bytes) {
        Ok(meta) => Ok(ok(image_meta_json(&meta))),
        Err(app_state::ImageStoreError::TooLarge) => {
            Err(ApiError::TooLarge("图片过大（上限 10MB）"))
        }
        Err(app_state::ImageStoreError::TotalExceeded) => {
            Err(ApiError::TooLarge("图片总量超限（上限 50MB），请先删除旧图"))
        }
        Err(app_state::ImageStoreError::BadFormat(_e)) => Err(ApiError::BadFormat),
        Err(app_state::ImageStoreError::Io(e)) => Err(ApiError::Internal(e)),
    }
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// ImageMeta → 响应 JSON
fn image_meta_json(m: &crate::config::ImageMeta) -> serde_json::Value {
    json!({
        "id": m.id,
        "fileName": m.file_name,
        "width": m.width,
        "height": m.height,
        "sizeBytes": m.size_bytes,
        "inUse": m.in_use,
        "addedAt": m.added_at,
    })
}

/// GET /api/v1/images：图片列表（含统计字段）
pub async fn list_images(State(state): State<SharedState>) -> Response {
    let inner = state.inner.lock().unwrap();
    let images: Vec<serde_json::Value> = inner.config.images.iter().map(image_meta_json).collect();
    let total: u64 = inner.config.images.iter().map(|m| m.size_bytes).sum();
    ok(json!({
        "images": images,
        "totalBytes": total,
        "maxTotalBytes": 50 * 1024 * 1024,
        "maxFileBytes": 10 * 1024 * 1024,
    }))
}

/// DELETE /api/v1/images/{id}：删除图片（使用中的删除后自动回退 clock）
pub async fn delete_image(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    // id 白名单校验（服务端生成格式 img_hex，双重防御路径穿越）
    if !is_safe_image_id(&id) {
        return Err(ApiError::InvalidParam);
    }
    match state.delete_image(&id, &src_of(&addr)) {
        Ok(true) => Ok(ok(json!({ "deleted": id }))),
        Ok(false) => Err(ApiError::InvalidParam),
        Err(e) => Err(ApiError::Internal(e)),
    }
}

/// 图片 id 合法性：img_ + 8 位 hex（服务端生成格式）
fn is_safe_image_id(id: &str) -> bool {
    id.len() == 12
        && id.starts_with("img_")
        && id[4..].chars().all(|c| c.is_ascii_hexdigit())
}

/// GET /api/v1/images/{id}/raw：图片原始字节（缩略图展示；带鉴权头 fetch 后 blob 展示）
pub async fn image_raw(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    if !is_safe_image_id(&id) {
        return Err(ApiError::InvalidParam);
    }
    match state.image_raw(&id) {
        Some((bytes, mime)) => Ok((
            [
                (header::CONTENT_TYPE, mime),
                (header::CACHE_CONTROL, "private, max-age=60"),
            ],
            bytes,
        )
            .into_response()),
        None => Err(ApiError::InvalidParam),
    }
}

/// PUT /api/v1/brightness 请求体
#[derive(Deserialize)]
pub struct BrightnessReq {
    /// 亮度 0-100
    pub value: u8,
}

/// PUT /api/v1/brightness：亮度（P1；一期固定返回不支持 50302，控制台隐藏滑杆）
pub async fn set_brightness(Json(_req): Json<BrightnessReq>) -> Response {
    ApiError::NotSupported.into_response()
}

/// GET /api/v1/config 可下发配置
#[derive(Deserialize, Default)]
pub struct RemoteConfig {
    /// 默认模式（可缺省=不改）
    #[serde(default)]
    pub default_mode: Option<String>,
    /// 时间制式 true=12h
    #[serde(default)]
    pub hour_12: Option<bool>,
    /// 图片遮罩百分比 0-60
    #[serde(default)]
    pub image_mask_percent: Option<u8>,
}

/// GET /api/v1/config：读取可远程下发的设置（F-P0-15）
pub async fn get_config(State(state): State<SharedState>) -> Response {
    let inner = state.inner.lock().unwrap();
    ok(json!({
        "defaultMode": inner.config.default_mode.as_str(),
        "hour12": inner.config.hour_12,
        "imageMaskPercent": inner.config.image_mask_percent,
    }))
}

/// PUT /api/v1/config：修改可远程下发的设置
pub async fn set_config(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(req): Json<RemoteConfig>,
) -> Result<Response, ApiError> {
    // ////////////////  校验参数  start  //////////////////////////////////////
    let mode = match &req.default_mode {
        Some(s) => Some(Mode::parse(s).ok_or(ApiError::InvalidParam)?),
        None => None,
    };
    if let Some(m) = req.image_mask_percent {
        if m > 60 {
            return Err(ApiError::InvalidParam);
        }
    }
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    {
        let mut inner = state.inner.lock().unwrap();
        if let Some(m) = mode {
            // 与 PUT /mode 相同约束：image 需已有图片
            if m == Mode::Image && inner.config.images.is_empty() {
                return Err(ApiError::NoImages);
            }
            inner.config.default_mode = m;
            if inner.locked {
                inner.locked_mode = m;
            }
        }
        if let Some(h) = req.hour_12 {
            inner.config.hour_12 = h;
        }
        if let Some(m) = req.image_mask_percent {
            inner.config.image_mask_percent = m;
        }
    }
    state.save_config();
    // 模式变化需通知锁定窗口重渲染
    if mode.is_some() {
        let img = state.active_image_path();
        let mode_now = { state.inner.lock().unwrap().locked_mode };
        let _ = state.app.emit_to(
            crate::lock::LOCK_MAIN_LABEL,
            "rsl://mode-changed",
            json!({ "mode": mode_now.as_str(), "imagePath": img }),
        );
    }
    state.log(&src_of(&addr), "PUT /config", "ok");
    let inner = state.inner.lock().unwrap();
    Ok(ok(json!({
        "defaultMode": inner.config.default_mode.as_str(),
        "hour12": inner.config.hour_12,
        "imageMaskPercent": inner.config.image_mask_percent,
    })))
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}
