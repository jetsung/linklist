//! 反向代理认证中间件：对齐上游 `middlewares.py::CustomRemoteUserMiddleware`
//! + `settings/base.py` 的 `LD_ENABLE_AUTH_PROXY` 逻辑。
//!
//! 反代在可信网络侧注入用户名请求头（默认 `REMOTE_USER`）；本中间件据此
//! 自动登录（不存在则建号），无需密码。仅当 `enable_auth_proxy` 开启时生效。

use crate::db::DbPool;
use axum::http::Request;
use tower_sessions::Session;

use crate::models;
use crate::web::auth_api::{SESSION_USER_KEY, current_user};
use crate::{error::AppError, models::audit_action};

/// 代理认证上下文：命中后写入 request extensions，由 handler/内层中间件落会话。
#[allow(dead_code)] // 预留：认证代理链路后续接入
#[derive(Debug, Clone)]
pub struct ProxyUser {
    pub username: String,
}

/// 从请求头提取代理用户名。
pub fn proxy_username<B>(req: &Request<B>, header_name: &str) -> Option<String> {
    let value = req.headers().get(header_name)?;
    let username = value.to_str().ok()?.trim();
    if username.is_empty() {
        None
    } else {
        Some(username.to_string())
    }
}

/// 反向代理认证中间件。
///
/// - `enable_auth_proxy` 关闭：直接放行（中间件自移除语义）。
/// - 开启且携带用户名头：自动登录/建号后放行。
/// - 开启但无头：匿名放行（与上游 RemoteUserMiddleware 一致，由后续授权层拒绝）。
pub async fn auth_proxy_middleware(
    axum::extract::State(state): axum::extract::State<crate::web::auth_api::AppState>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, AppError> {
    if !state.config.enable_auth_proxy {
        return Ok(next.run(req).await);
    }
    let header_name = state.config.auth_proxy_username_header.as_str();
    if let Some(username) = proxy_username(&req, header_name) {
        let session = req
            .extensions_mut()
            .get::<Session>()
            .cloned()
            .ok_or_else(|| AppError::internal("会话中间件未挂载"))?;
        login_proxy_user(&state.pool, &session, &username).await?;
    }
    Ok(next.run(req).await)
}

/// 按用户名取用户；不存在则创建（自动建号，对齐上游 RemoteUserMiddleware 行为）。
/// 首个账号即超级用户：库中无用户时建号自动授管理员。
pub async fn ensure_user(pool: &DbPool, username: &str) -> Result<models::User, AppError> {
    if let Some(user) = models::find_by_username(pool, username).await? {
        return Ok(user);
    }
    // 随机口令：代理用户不经密码登录，仅占位满足 NOT NULL 约束。
    let mut bytes = [0u8; 16];
    rand::fill(&mut bytes);
    let random_password: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    models::create_user_auto_admin(pool, username, &random_password).await
}

/// 将代理用户写入会话（幂等：已登录同一用户则跳过）。
pub async fn login_proxy_user(
    pool: &DbPool,
    session: &Session,
    username: &str,
) -> Result<models::User, AppError> {
    if let Some(existing) = current_user(pool, session).await?
        && existing.username == username
    {
        return Ok(existing);
    }
    let user = ensure_user(pool, username).await?;
    if !user.is_active {
        return Err(AppError::Unauthorized);
    }
    session
        .insert(SESSION_USER_KEY, user.id)
        .await
        .map_err(AppError::internal)?;
    crate::models::audit(pool, Some(user.id), audit_action::LOGIN, "auth_proxy").await;
    Ok(user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ensure_user_creates_and_reuses() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        // 空库首用户 → 管理员；重复取同一用户幂等。
        let u1 = ensure_user(&pool, "proxyuser").await.unwrap();
        assert!(u1.is_admin, "空库首个 SSO 建号用户应为管理员");
        let u2 = ensure_user(&pool, "proxyuser").await.unwrap();
        assert_eq!(u1.id, u2.id);
        assert_eq!(u1.username, "proxyuser");
        assert!(u2.is_admin);
        // 已有用户后新建的 SSO 用户 → 普通用户。
        let u3 = ensure_user(&pool, "plainuser").await.unwrap();
        assert!(!u3.is_admin, "非首用户应为普通用户");
    }

    #[test]
    fn proxy_username_reads_configured_header() {
        let req = Request::builder()
            .header("X-WebAuth-User", "alice")
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(
            proxy_username(&req, "X-WebAuth-User").as_deref(),
            Some("alice")
        );
        assert_eq!(proxy_username(&req, "REMOTE_USER"), None);
    }
}
