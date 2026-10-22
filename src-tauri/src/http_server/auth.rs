//! Bearer token 鉴权中间件（F-P0-05 / S-02）。
//! - 除 /healthz、/api/v1/pair 外全部 /api/v1/* 接口需鉴权
//! - 无 token → 401 40101；错 token → 401 40102
//! - 同 IP 鉴权失败 10 次/分钟 → 429 42901（附重试等待秒数）

use crate::app_state::SharedState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;

/// 鉴权中间件（挂在受保护路由组上）
pub async fn auth_middleware(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    // ////////////////  准备数据：解析 Bearer token  start  ////////////////////
    let token = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string());
    // ////////////////  核心逻辑  start  //////////////////////////////////////
    match token {
        Some(t) if state.token_valid(&t) => next.run(req).await,
        Some(_) => {
            // 提供了 token 但无效 → 40102（先检查限速）
            fail_response(&state, addr, true)
        }
        None => {
            // 未提供 token → 40101
            fail_response(&state, addr, false)
        }
    }
    // ////////////////  核心逻辑  end  ////////////////////////////////////////
}

/// 构造鉴权失败响应（含限速判定）
fn fail_response(state: &SharedState, addr: SocketAddr, had_token: bool) -> Response {
    let (limited, retry_after) = state.record_auth_failure(addr.ip());
    let err = if limited {
        super::api::ApiError::TooManyAttempts { retry_after_sec: retry_after }
    } else if had_token {
        super::api::ApiError::InvalidToken
    } else {
        super::api::ApiError::MissingToken
    };
    err.into_response()
}
