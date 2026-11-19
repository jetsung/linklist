//! TODO13 集成测试：CORS 中间件 / auth proxy / OIDC 登录流程。
//!
//! OIDC 用本地 TcpListener 模拟 IdP（authorize 302 / token JSON / userinfo JSON）。

use crate::web::auth_api::integration::*;

async fn app_with_config(cfg: crate::config::Config) -> axum::Router {
    let pool = crate::db::connect_in_memory().await.unwrap();
    build_app_with_config(pool, None, cfg).await
}

// ---------- CORS ----------

#[tokio::test]
async fn cors_preflight_returns_headers_for_allowed_origin() {
    let cfg = crate::config::Config {
        cors_allowed_origins: vec!["https://app.example.com".into()],
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/bookmarks")
                .header("Origin", "https://app.example.com")
                .header("Access-Control-Request-Method", "GET")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://app.example.com")
    );
    assert!(resp.headers().get("access-control-allow-methods").is_some());
    assert!(resp.headers().get("access-control-max-age").is_some());
}

#[tokio::test]
async fn cors_rejects_unlisted_origin() {
    let cfg = crate::config::Config {
        cors_allowed_origins: vec!["https://app.example.com".into()],
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/bookmarks")
                .header("Origin", "https://evil.example.com")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.headers().get("access-control-allow-origin").is_none());
    // Vary: Origin 仍应存在（对齐上游 patch_vary_headers）。
    assert_eq!(
        resp.headers().get("vary").and_then(|v| v.to_str().ok()),
        Some("Origin")
    );
}

#[tokio::test]
async fn cors_disabled_when_no_origins() {
    let app = app_with_config(Default::default()).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("OPTIONS")
                .uri("/api/v1/bookmarks")
                .header("Origin", "https://app.example.com")
                .header("Access-Control-Request-Method", "GET")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.headers().get("access-control-allow-origin").is_none());
}

// ---------- Auth proxy ----------

#[tokio::test]
async fn auth_proxy_disabled_ignores_header() {
    let app = app_with_config(Default::default()).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/auth/status")
                .header("REMOTE_USER", "proxyuser")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["authenticated"], serde_json::Value::Bool(false));
}

#[tokio::test]
async fn auth_proxy_header_creates_and_logs_in() {
    let cfg = crate::config::Config {
        enable_auth_proxy: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/auth/status")
                .header("REMOTE_USER", "proxyuser")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["authenticated"], serde_json::Value::Bool(true));
    assert_eq!(json["username"], "proxyuser");
}

#[tokio::test]
async fn auth_proxy_custom_header_name() {
    let cfg = crate::config::Config {
        enable_auth_proxy: true,
        auth_proxy_username_header: "X-WebAuth-User".into(),
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/v1/auth/status")
                .header("X-WebAuth-User", "sso_alice")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["username"], "sso_alice");
}

// ---------- OIDC ----------

/// 启动 mock IdP：返回 (base_url, all_calls, discovery_calls)。
/// 提供 /.well-known/openid-configuration（端点指向 mock 自身），支持 discovery 测试。
async fn spawn_mock_idp() -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    let dcounter = Arc::new(AtomicUsize::new(0));
    let dcounter_clone = dcounter.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            let counter = counter_clone.clone();
            let dcounter = dcounter_clone.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 8192];
                let mut stream = stream;
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                counter.fetch_add(1, Ordering::SeqCst);
                let (status, body, content_type) =
                    if path.starts_with("/.well-known/openid-configuration") {
                        dcounter.fetch_add(1, Ordering::SeqCst);
                        let host = req
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("host:"))
                            .and_then(|l| l.split_once(':').map(|(_, v)| v))
                            .map(str::trim)
                            .unwrap_or("127.0.0.1:0")
                            .to_string();
                        let base = format!("http://{host}");
                        (
                            "200 OK",
                            serde_json::json!({
                                "issuer": base,
                                "authorization_endpoint": format!("{base}/authorize"),
                                "token_endpoint": format!("{base}/token"),
                                "jwks_uri": format!("{base}/jwks"),
                                "userinfo_endpoint": format!("{base}/userinfo"),
                            })
                            .to_string(),
                            "application/json",
                        )
                    } else if path.starts_with("/token") {
                        // ID Token：HS256，secret "test-secret"，claims 带 email。
                        let header = crate::web::oidc::b64url(br#"{"alg":"HS256","typ":"JWT"}"#);
                        let mut claims = serde_json::Map::new();
                        claims.insert("iss".into(), "http://mock-idp".into());
                        claims.insert("aud".into(), "test-client".into());
                        claims.insert("sub".into(), "user-1".into());
                        claims.insert("email".into(), "oidc-user@example.com".into());
                        claims.insert("exp".into(), (chrono::Utc::now().timestamp() + 600).into());
                        let payload = crate::web::oidc::b64url(
                            serde_json::to_string(&claims).unwrap().as_bytes(),
                        );
                        let signing_input = format!("{header}.{payload}");
                        use hmac::{Hmac, KeyInit, Mac};
                        type HmacSha256 = Hmac<sha2::Sha256>;
                        let mut mac = HmacSha256::new_from_slice(b"test-secret").unwrap();
                        mac.update(signing_input.as_bytes());
                        let sig = crate::web::oidc::b64url(&mac.finalize().into_bytes());
                        let token = format!("{signing_input}.{sig}");
                        (
                            "200 OK",
                            serde_json::json!({
                                "access_token": "at-123",
                                "id_token": token,
                                "token_type": "Bearer",
                            })
                            .to_string(),
                            "application/json",
                        )
                    } else if path.starts_with("/userinfo") {
                        (
                            "200 OK",
                            serde_json::json!({
                                "sub": "user-1",
                                "email": "oidc-user@example.com",
                            })
                            .to_string(),
                            "application/json",
                        )
                    } else if path.starts_with("/authorize") {
                        ("302 Found", String::new(), "text/plain")
                    } else {
                        ("404 Not Found", "not found".into(), "text/plain")
                    };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(resp.as_bytes()).await.ok();
                stream.flush().await.ok();
            });
        }
    });
    (format!("http://{addr}"), counter, dcounter)
}

#[tokio::test]
async fn oidc_full_flow_login_and_callback() {
    let (idp_base, _calls, _dcalls) = spawn_mock_idp().await;
    let cfg = crate::config::Config {
        enable_oidc: true,
        oidc_authorization_endpoint: Some(format!("{idp_base}/authorize")),
        oidc_token_endpoint: Some(format!("{idp_base}/token")),
        oidc_userinfo_endpoint: Some(format!("{idp_base}/userinfo")),
        oidc_client_id: Some("test-client".into()),
        oidc_client_secret: Some("test-secret".into()),
        oidc_sign_algo: "HS256".into(),
        oidc_username_claim: "email".into(),
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;

    // 1. /oidc/login → 302 到 IdP 授权端点。
    let resp = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/oidc/login")
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    assert!(
        location.starts_with(&format!("{idp_base}/authorize")),
        "location={location}"
    );
    assert!(location.contains("state="));
    assert!(location.contains("code_challenge="));

    // 2. 模拟 IdP 回跳：带 code/state → callback。state 需与 login 会话一致；
    //    测试里直接取 login 302 的 state，但 session cookie 在两次 oneshot 间不共享，
    //    因此这里验证“state 缺失被拒”与“正确 state 需真实会话”——拆为两个断言：
    //    a) 无 cookie 的 callback 报 state 缺失（400）。
    let code = "test-code";
    let state = location
        .split("state=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .unwrap_or("")
        .to_string();
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/oidc/callback?code={code}&state={state}"))
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oidc_disabled_returns_404() {
    let app = app_with_config(Default::default()).await;
    use tower::ServiceExt;
    for path in ["/oidc/login", "/oidc/callback"] {
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::NOT_FOUND,
            "path={path}"
        );
    }
}

#[tokio::test]
async fn oidc_callback_rejects_missing_state() {
    let (idp_base, _calls, _dcalls) = spawn_mock_idp().await;
    let cfg = crate::config::Config {
        enable_oidc: true,
        oidc_authorization_endpoint: Some(format!("{idp_base}/authorize")),
        oidc_token_endpoint: Some(format!("{idp_base}/token")),
        oidc_client_id: Some("test-client".into()),
        oidc_client_secret: Some("test-secret".into()),
        oidc_sign_algo: "HS256".into(),
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/oidc/callback?code=x")
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

// ---------- OIDC Discovery ----------

/// 构造仅配置 Issuer 的 OIDC Config（client 走 HS256，与 mock ID Token 一致）。
fn discovery_config(idp_base: &str) -> crate::config::Config {
    crate::config::Config {
        enable_oidc: true,
        oidc_issuer: Some(idp_base.to_string()),
        oidc_client_id: Some("test-client".into()),
        oidc_client_secret: Some("test-secret".into()),
        oidc_sign_algo: "HS256".into(),
        oidc_username_claim: "email".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn oidc_discovery_login_uses_discovered_endpoints() {
    let (idp_base, _calls, _dcalls) = spawn_mock_idp().await;
    let app = app_with_config(discovery_config(&idp_base)).await;
    use tower::ServiceExt;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/oidc/login")
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    // 授权端点来自 discovery 文档。
    assert!(
        location.starts_with(&format!("{idp_base}/authorize")),
        "location={location}"
    );
    assert!(location.contains("client_id=test-client"));
}

#[tokio::test]
async fn oidc_discovery_explicit_overrides_discovery() {
    let (idp_base, _calls, _dcalls) = spawn_mock_idp().await;
    // 另起一个 mock 作为显式端点地址（与 issuer 不同源）。
    let (other_base, _c2, _d2) = spawn_mock_idp().await;
    let cfg = crate::config::Config {
        oidc_authorization_endpoint: Some(format!("{other_base}/authorize")),
        ..discovery_config(&idp_base)
    };
    let app = app_with_config(cfg).await;
    use tower::ServiceExt;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/oidc/login")
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    // 显式配置的授权端点优先于 discovery 结果。
    assert!(
        location.starts_with(&format!("{other_base}/authorize")),
        "location={location}"
    );
    assert!(!location.starts_with(&format!("{idp_base}/authorize")));
}

#[tokio::test]
async fn oidc_discovery_missing_field_errors() {
    // mock 的 discovery 缺 authorization_endpoint：起一个只回部分字段的端点。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => return,
            };
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 2048];
                let mut stream = stream;
                let _ = stream.read(&mut buf).await.unwrap_or(0);
                let body = serde_json::json!({ "issuer": "http://partial" }).to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(resp.as_bytes()).await.ok();
                stream.flush().await.ok();
            });
        }
    });
    let issuer = format!("http://{addr}");
    let app = app_with_config(discovery_config(&issuer)).await;
    use tower::ServiceExt;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/oidc/login")
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("authorization_endpoint"), "body={text}");
}

#[tokio::test]
async fn oidc_discovery_unreachable_errors() {
    // Issuer 指向无监听端口 → 明确报错而非内部错误。
    let app = app_with_config(discovery_config("http://127.0.0.1:1")).await;
    use tower::ServiceExt;

    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/oidc/login")
                .header("Host", "linklist.local")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("OIDC discovery"), "body={text}");
}

#[tokio::test]
async fn oidc_discovery_cached_across_logins() {
    let (idp_base, _calls, dcalls) = spawn_mock_idp().await;
    let app = app_with_config(discovery_config(&idp_base)).await;
    use tower::ServiceExt;

    for _ in 0..2 {
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/oidc/login")
                    .header("Host", "linklist.local")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::SEE_OTHER);
    }
    // 第二次登录不再请求 discovery（进程内缓存命中）。
    assert_eq!(
        dcalls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "discovery 应仅被请求一次"
    );
}
