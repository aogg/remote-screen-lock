//! 内置 HTTP 服务（F-P0-02 / F-P0-03）：axum 单端口同时提供
//! REST API（Bearer token）、手机控制台静态页与 PWA manifest。
//!
//! - 默认监听 0.0.0.0:8326；被占用时在 8326~8335 内自动重试，实际端口写入状态（设置窗提示）
//! - 服务异常退出时自动重启监听（R-02：注入崩溃后 5 秒内 /healthz 恢复）

pub mod api;
pub mod auth;
pub mod static_assets;

use crate::app_state::SharedState;
use axum::extract::{DefaultBodyLimit, Request};
use axum::http::{header, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::net::SocketAddr;

/// 端口重试区间（F-P0-02：8326~8335）
const PORT_MIN: u16 = 8326;
const PORT_MAX: u16 = 8335;

/// 单请求体积上限：12MB（图片上传 10MB + multipart 开销）
const BODY_LIMIT: usize = 12 * 1024 * 1024;

/// 构建完整 Router（API + 静态控制台）
pub fn build_router(state: SharedState) -> Router {
    // ////////////////  准备数据：受保护路由组  start  /////////////////////////
    let protected = Router::new()
        .route("/api/v1/pair/verify", post(api::pair_verify))
        .route("/api/v1/status", get(api::status))
        .route("/api/v1/lock", post(api::lock))
        .route("/api/v1/unlock", post(api::unlock))
        .route("/api/v1/screen/off", post(api::screen_off))
        .route("/api/v1/screen/on", post(api::screen_on))
        .route("/api/v1/mode", get(api::get_mode).put(api::set_mode))
        .route(
            "/api/v1/images",
            post(api::upload_image).get(api::list_images),
        )
        .route("/api/v1/images/{id}", delete(api::delete_image))
        .route("/api/v1/images/{id}/raw", get(api::image_raw))
        .route("/api/v1/brightness", put(api::set_brightness))
        .route("/api/v1/config", get(api::get_config).put(api::set_config))
        .route_layer(from_fn_with_state(state.clone(), auth::auth_middleware));
    // ////////////////  核心逻辑：组装  start  /////////////////////////////////
    Router::new()
        // 公开：健康检查（仅版本）+ 配对 + 静态控制台（/ 与 /console 同页）
        .route("/healthz", get(api::healthz))
        .route("/api/v1/pair", post(api::pair_submit))
        .merge(protected)
        .fallback(static_fallback)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .with_state(state)
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 未匹配路由 → 静态资源表命中则返回；否则 404
async fn static_fallback(req: Request) -> Response {
    let path = req.uri().path().to_string();
    match static_assets::lookup(&path) {
        Some(asset) => (
            [
                (header::CONTENT_TYPE, asset.mime),
                // 静态资源短缓存；断外网环境由本地打包保证可用
                (header::CACHE_CONTROL, "no-cache"),
            ],
            asset.bytes,
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            "404 Not Found（控制台页面见 / ，配对页见 /pair）",
        )
            .into_response(),
    }
}

/// HTTP 服务主循环（含端口重试 + 崩溃自动重启，R-02）
pub async fn run(state: SharedState) {
    // ////////////////  核心逻辑：监督循环  start  /////////////////////////////
    loop {
        let desired = state.inner.lock().unwrap().config.port;
        // 端口重试：desired 起在 PORT_MIN..=PORT_MAX 内依次尝试
        let start = desired.clamp(PORT_MIN, PORT_MAX);
        let mut bound: Option<(tokio::net::TcpListener, u16)> = None;
        for port in start..=PORT_MAX {
            match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
                Ok(listener) => {
                    bound = Some((listener, port));
                    break;
                }
                Err(e) => {
                    state.log("system", "http-bind", &format!("err:port {port} ({e})"));
                }
            }
        }
        let Some((listener, port)) = bound else {
            // 8326~8335 全占用：5 秒后重试（应用不退出，托盘仍可用）
            state.log("system", "http-bind", "err:all-ports-busy(8326-8335)");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            continue;
        };
        // 记录实际端口（配置端口被占用时与配置不同 → 设置窗提示）
        state.inner.lock().unwrap().actual_port = port;
        state.log("system", "http-listen", &format!("ok:0.0.0.0:{port}"));
        let app = build_router(state.clone());
        let service = app.into_make_service_with_connect_info::<SocketAddr>();
        // serve 正常情况下不返回；返回即视为服务异常退出 → 记录后重启
        if let Err(e) = axum::serve(listener, service).await {
            state.log("system", "http-serve", &format!("err:{e}"));
        }
        // 崩溃恢复：1 秒后重绑（R-02：5 秒内恢复）
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

