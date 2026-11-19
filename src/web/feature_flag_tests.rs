//! 部署开关集成测试：对齐上游 5 个环境开关。
//!
//! 覆盖：LD_DISABLE_URL_VALIDATION / LD_ENABLE_SNAPSHOTS /
//! LD_DISABLE_ASSET_UPLOAD / LD_DISABLE_LOGIN_FORM / LD_DISABLE_BACKGROUND_TASKS。

use crate::web::auth_api::integration::*;
use axum::http::StatusCode;
use tower::ServiceExt;

async fn app_with_config(cfg: crate::config::Config) -> axum::Router {
    let pool = crate::db::connect_in_memory().await.unwrap();
    build_app_with_config(pool, None, cfg).await
}

/// 建号 + 登录拿 cookie（走 /api/v1/auth/setup + /api/v1/auth/login）。
async fn setup_login(app: &axum::Router) -> String {
    login_cookie(app).await
}

async fn get_json(app: &axum::Router, uri: &str, cookie: &str) -> (StatusCode, serde_json::Value) {
    let req = axum::http::Request::builder()
        .uri(uri)
        .header("cookie", cookie)
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap_or_default();
    let v = if body.is_empty() {
        serde_json::json!(null)
    } else {
        serde_json::from_slice(&body).unwrap_or(serde_json::json!(null))
    };
    (status, v)
}

// ---------- LD_DISABLE_LOGIN_FORM ----------

#[tokio::test]
async fn disable_login_form_blocks_login_api() {
    let cfg = crate::config::Config {
        disable_login_form: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    let (status, _, _) = post_json(
        &app,
        "/api/v1/auth/login",
        r#"{"username":"admin","password":"password123"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn disable_login_form_still_allows_setup() {
    let cfg = crate::config::Config {
        disable_login_form: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    let (status, _, _) = post_json(
        &app,
        "/api/v1/auth/setup",
        r#"{"username":"admin","password":"password123"}"#,
    )
    .await;
    assert!(status.is_success());
}

#[tokio::test]
async fn auth_status_exposes_login_flags() {
    let cfg = crate::config::Config {
        disable_login_form: true,
        enable_oidc: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    let (status, body) = get_json(&app, "/api/v1/auth/status", "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["disable_login_form"], serde_json::json!(true));
    assert_eq!(body["enable_oidc"], serde_json::json!(true));
}

// ---------- LD_DISABLE_ASSET_UPLOAD ----------

#[tokio::test]
async fn disable_asset_upload_blocks_asset_endpoints() {
    let cfg = crate::config::Config {
        disable_asset_upload: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    let cookie = setup_login(&app).await;
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"https://example.com/a"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "创建书签应成功");
    let (status, _) = get_json(&app, "/api/v1/bookmarks/1/assets", &cookie).await;
    // GET 不受影响；POST /assets 与 POST singlefile 受 403。
    assert_ne!(status, StatusCode::FORBIDDEN, "GET 列表不应被资产开关拦截");
    let (status, _, _) =
        post_with_cookie(&app, "/api/v1/bookmarks/1/assets", "", Some(&cookie)).await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST,
        "禁用后资产上传应 403/400，实际 {status}"
    );
    let (status, _, _) =
        post_with_cookie(&app, "/api/v1/bookmarks/1/singlefile", "", Some(&cookie)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "singlefile 上传应 403");
}

// ---------- LD_ENABLE_SNAPSHOTS ----------

#[tokio::test]
async fn snapshots_disabled_blocks_bulk_snapshot() {
    let app = app_with_config(crate::config::Config::default()).await;
    let cookie = setup_login(&app).await;
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"https://example.com/s"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks/bulk/snapshot",
        r#"{"ids":[1]}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "未开 enable_snapshots 应拒绝"
    );
}

// ---------- LD_DISABLE_BACKGROUND_TASKS ----------

#[tokio::test]
async fn disable_background_tasks_blocks_manual_triggers() {
    let cfg = crate::config::Config {
        enable_snapshots: true,
        disable_background_tasks: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    let cookie = setup_login(&app).await;
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"https://example.com/b"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks/bulk/refresh",
        r#"{"ids":[1]}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks/bulk/snapshot",
        r#"{"ids":[1]}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks/1/capture",
        r#"{"kind":"metadata"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---------- LD_DISABLE_URL_VALIDATION ----------

#[tokio::test]
async fn default_validates_bookmark_url() {
    let app = app_with_config(crate::config::Config::default()).await;
    let cookie = setup_login(&app).await;
    let (status, body, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"ftp://example.com/x"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "ftp 应被拒: {body}");
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"https://example.com/ok"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "https 应通过");
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"example.com/noscheme"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "补 https 后应通过");
}

#[tokio::test]
async fn disable_url_validation_skips_check() {
    let cfg = crate::config::Config {
        disable_url_validation: true,
        ..Default::default()
    };
    let app = app_with_config(cfg).await;
    let cookie = setup_login(&app).await;
    let (status, _, _) = post_with_cookie(
        &app,
        "/api/v1/bookmarks",
        r#"{"url":"ftp://example.com/x"}"#,
        Some(&cookie),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "关闭校验后 ftp 放行");
    let (status, _) = get_json(&app, "/api/v1/bookmarks/check?url=notaurl", &cookie).await;
    assert_eq!(status, StatusCode::OK, "关闭校验后 check 放行");
}
