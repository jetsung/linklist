pub mod admin_api;
pub mod assets_api;
pub mod audit_api;
pub mod auth_api;
pub mod auth_proxy;
pub mod bookmarks_api;
pub mod bundle_api;
pub mod capture_api;
pub mod cors;
pub mod favicon;
pub mod feed;
pub mod frontend;
pub mod import_export;
pub mod oidc;
pub mod preview;
pub mod profile_api;
pub mod shared_api;

#[cfg(test)]
mod feature_flag_tests;
#[cfg(test)]
mod oidc_tests;

use axum::Router;
use axum::extract::State;
use axum::response::IntoResponse;

use crate::web::auth_api::AppState;

/// 组装 API 路由（/api/v1）。
pub fn api_router(state: AppState) -> Router {
    Router::new()
        .route("/oidc/login", axum::routing::get(oidc::oidc_login))
        .route("/oidc/callback", axum::routing::get(oidc::oidc_callback))
        .route("/api/v1/auth/status", axum::routing::get(auth_api::status))
        .route("/api/v1/auth/setup", axum::routing::post(auth_api::setup))
        .route("/api/v1/auth/login", axum::routing::post(auth_api::login))
        .route("/api/v1/auth/logout", axum::routing::post(auth_api::logout))
        .route("/api/v1/auth/me", axum::routing::get(auth_api::me))
        .route(
            "/api/v1/auth/password",
            axum::routing::post(auth_api::change_password),
        )
        .route(
            "/api/v1/auth/tokens",
            axum::routing::post(auth_api::create_token).get(auth_api::list_tokens),
        )
        .route(
            "/api/v1/auth/tokens/{id}",
            axum::routing::delete(auth_api::revoke_token),
        )
        .route(
            "/api/v1/bookmarks/bulk/tags/add",
            axum::routing::post(bookmarks_api::bulk_add_tags),
        )
        .route(
            "/api/v1/bookmarks/bulk/tags/remove",
            axum::routing::post(bookmarks_api::bulk_remove_tags),
        )
        .route(
            "/api/v1/bookmarks/bulk/archive",
            axum::routing::post(bookmarks_api::bulk_archive),
        )
        .route(
            "/api/v1/bookmarks/bulk/unarchive",
            axum::routing::post(bookmarks_api::bulk_unarchive),
        )
        .route(
            "/api/v1/bookmarks/bulk/mark-read",
            axum::routing::post(bookmarks_api::bulk_mark_read),
        )
        .route(
            "/api/v1/bookmarks/bulk/mark-unread",
            axum::routing::post(bookmarks_api::bulk_mark_unread),
        )
        .route(
            "/api/v1/bookmarks/bulk/delete",
            axum::routing::post(bookmarks_api::bulk_delete),
        )
        .route(
            "/api/v1/bookmarks/bulk/share",
            axum::routing::post(bookmarks_api::bulk_share),
        )
        .route(
            "/api/v1/bookmarks/bulk/unshare",
            axum::routing::post(bookmarks_api::bulk_unshare),
        )
        .route(
            "/api/v1/bookmarks/bulk/refresh",
            axum::routing::post(bookmarks_api::bulk_refresh),
        )
        .route(
            "/api/v1/bookmarks/bulk/snapshot",
            axum::routing::post(bookmarks_api::bulk_snapshot),
        )
        .route(
            "/api/v1/bookmarks",
            axum::routing::get(bookmarks_api::list).post(bookmarks_api::create),
        )
        .route(
            "/api/v1/bookmarks/check",
            axum::routing::get(bookmarks_api::check),
        )
        .route(
            "/api/v1/bookmarks/tags",
            axum::routing::get(bookmarks_api::list_tags).post(bookmarks_api::create_tag),
        )
        .route(
            "/api/v1/bookmarks/tags/merge",
            axum::routing::post(bookmarks_api::merge_tags),
        )
        .route(
            "/api/v1/bookmarks/tags/{name}",
            axum::routing::get(bookmarks_api::get_tag)
                .patch(bookmarks_api::rename_tag)
                .delete(bookmarks_api::delete_tag),
        )
        .route(
            "/api/v1/bookmarks/tags/complete",
            axum::routing::get(bookmarks_api::complete_tags),
        )
        .route(
            "/api/v1/bookmarks/{id}",
            axum::routing::get(bookmarks_api::get_one)
                .patch(bookmarks_api::update)
                .delete(bookmarks_api::delete),
        )
        .route(
            "/api/v1/bookmarks/{id}/tags",
            axum::routing::put(bookmarks_api::set_tags),
        )
        .route(
            "/api/v1/bookmarks/{id}/archive",
            axum::routing::post(bookmarks_api::archive),
        )
        .route(
            "/api/v1/bookmarks/{id}/unarchive",
            axum::routing::post(bookmarks_api::unarchive),
        )
        .route(
            "/api/v1/bookmarks/import",
            axum::routing::post(import_export::import),
        )
        .route(
            "/api/v1/bookmarks/export",
            axum::routing::get(import_export::export),
        )
        .route(
            "/api/v1/profile/auto-tagging-rules",
            axum::routing::get(profile_api::get_rules).put(profile_api::set_rules),
        )
        .route(
            "/api/v1/profile",
            axum::routing::get(profile_api::get_preferences).put(profile_api::set_preferences),
        )
        .route(
            "/api/v1/profile/search-preferences",
            axum::routing::get(profile_api::get_search_preferences)
                .put(profile_api::set_search_preferences),
        )
        .route(
            "/api/v1/profile/feed-key",
            axum::routing::post(profile_api::create_feed_key),
        )
        .route(
            "/api/v1/profile/feed-key/regenerate",
            axum::routing::post(profile_api::regenerate_feed_key),
        )
        .route(
            "/api/v1/profile/refresh-favicons",
            axum::routing::post(profile_api::refresh_favicons),
        )
        .route("/api/v1/version", axum::routing::get(version))
        .route("/feed/{kind}", axum::routing::get(feed::feed))
        .route(
            "/feeds/shared",
            axum::routing::get(feed::feed_public_shared),
        )
        .route("/feeds/{key}/{kind}", axum::routing::get(feed::feed_keyed))
        .route("/health", axum::routing::get(health))
        .route("/opensearch.xml", axum::routing::get(opensearch))
        .route("/manifest.json", axum::routing::get(manifest))
        .route("/custom_css", axum::routing::get(custom_css))
        .route(
            "/api/v1/bundles/preview",
            axum::routing::get(bundle_api::preview),
        )
        .route(
            "/api/v1/bundles/preview/tags",
            axum::routing::get(bundle_api::preview_tags),
        )
        .route(
            "/api/v1/bundles",
            axum::routing::get(bundle_api::list_bundles).post(bundle_api::create_bundle),
        )
        .route(
            "/api/v1/bundles/{id}",
            axum::routing::get(bundle_api::get_bundle)
                .patch(bundle_api::update_bundle)
                .delete(bundle_api::delete_bundle),
        )
        .route(
            "/api/v1/bundles/reorder",
            axum::routing::post(bundle_api::reorder_bundles),
        )
        .route(
            "/api/v1/bundles/{id}/bookmarks",
            axum::routing::get(bundle_api::bundle_bookmarks),
        )
        .route(
            "/api/v1/bookmarks/{id}/snapshot",
            axum::routing::get(capture_api::get_snapshot),
        )
        .route(
            "/api/v1/bookmarks/{id}/singlefile",
            axum::routing::post(capture_api::upload_singlefile),
        )
        .route(
            "/api/v1/bookmarks/{id}/capture",
            axum::routing::post(capture_api::trigger_capture),
        )
        .route(
            "/api/v1/bookmarks/{id}/capture-tasks",
            axum::routing::get(capture_api::list_tasks),
        )
        .route(
            "/api/v1/capture-tasks/{task_id}/retry",
            axum::routing::post(capture_api::retry_task),
        )
        .route(
            "/api/v1/bookmarks/{id}/assets",
            axum::routing::post(assets_api::upload_asset).get(assets_api::list_assets),
        )
        .route(
            "/api/v1/bookmarks/{id}/assets/{aid}",
            axum::routing::get(assets_api::download_asset).delete(assets_api::delete_asset),
        )
        .route(
            "/api/v1/bookmarks/{id}/assets/{aid}/read",
            axum::routing::get(assets_api::read_asset),
        )
        .route(
            "/api/v1/admin/users",
            axum::routing::get(admin_api::list_users).post(admin_api::create_user),
        )
        .route(
            "/api/v1/admin/users/{id}/deactivate",
            axum::routing::post(admin_api::deactivate_user),
        )
        .route(
            "/api/v1/admin/users/{id}/activate",
            axum::routing::post(admin_api::activate_user),
        )
        .route(
            "/api/v1/admin/users/{id}/reset-password",
            axum::routing::post(admin_api::reset_password),
        )
        .route(
            "/api/v1/admin/data/bookmarks",
            axum::routing::get(admin_api::list_all_bookmarks),
        )
        .route(
            "/api/v1/admin/tasks",
            axum::routing::get(admin_api::list_tasks).delete(admin_api::clear_tasks),
        )
        .route(
            "/api/v1/admin/settings",
            axum::routing::get(admin_api::get_settings).put(admin_api::set_setting),
        )
        .route(
            "/api/v1/admin/audit-logs",
            axum::routing::get(audit_api::list_audit_logs).delete(audit_api::clear_audit_logs),
        )
        .route("/", frontend::page_route("/"))
        .route("/login", frontend::page_route("/login"))
        .route("/bookmarks", frontend::page_route("/bookmarks"))
        .route("/bookmarks/new", frontend::page_route("/bookmarks/new"))
        .route("/bookmarks/edit", frontend::page_route("/bookmarks/edit"))
        .route(
            "/bookmarks/archived",
            frontend::page_route("/bookmarks/archived"),
        )
        .route(
            "/bookmarks/shared",
            frontend::page_route("/bookmarks/shared"),
        )
        .route("/shared", frontend::page_route("/shared"))
        .route("/tags", frontend::page_route("/tags"))
        .route("/bundles", frontend::page_route("/bundles"))
        .route(
            "/bundles/{id}/edit",
            axum::routing::get(frontend::bundles_edit_page),
        )
        .route("/settings", frontend::page_route("/settings"))
        .route(
            "/settings/general",
            frontend::page_route("/settings/general"),
        )
        .route(
            "/settings/integrations",
            frontend::page_route("/settings/integrations"),
        )
        .route("/admin", frontend::page_route("/admin"))
        .route("/favicon/{host}", axum::routing::get(favicon::favicon))
        .route("/previews/{id}", axum::routing::get(preview::preview_image))
        .route("/assets/{*path}", axum::routing::get(frontend::asset))
        .route("/{*file}", axum::routing::get(frontend::static_file))
        .route(
            "/api/v1/shared",
            axum::routing::get(shared_api::list_shared),
        )
        .route(
            "/api/v1/shared/tags",
            axum::routing::get(shared_api::list_shared_tags),
        )
        .route(
            "/api/v1/shared/users",
            axum::routing::get(shared_api::list_shared_users),
        )
        .route(
            "/api/v1/shared/public",
            axum::routing::get(shared_api::list_public_shared),
        )
        .route(
            "/api/v1/shared/public/tags",
            axum::routing::get(shared_api::list_public_shared_tags),
        )
        .route(
            "/api/v1/shared/viewer-profile",
            axum::routing::get(shared_api::viewer_profile),
        )
        .route(
            "/api/v1/global-settings",
            axum::routing::get(profile_api::public_global_settings),
        )
        .layer(cors::CorsLayer::new(&state.config.cors_allowed_origins))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth_proxy::auth_proxy_middleware,
        ))
        .with_state(state)
}

/// `GET /api/v1/version`：返回应用版本（页面「关于」区块用）。
async fn version() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
}

/// `GET /health`：健康检查（对齐上游健康端点，匿名可访问）。
/// 返回 `{version, status}`；数据库 ping 失败 → `{status: "unhealthy"}` + 500。
async fn health(State(state): State<AppState>) -> axum::response::Response {
    let ok = crate::db::q("SELECT 1").execute(&state.pool).await.is_ok();
    let status = if ok { "healthy" } else { "unhealthy" };
    let code = if ok {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    };
    (
        code,
        axum::Json(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "status": status,
        })),
    )
        .into_response()
}

/// `GET /opensearch.xml`：OpenSearch 描述文档（对齐上游 opensearch 视图，匿名可访问）。
/// base_url 由请求 Host 头构造（与跳转 Location 同策略）。
async fn opensearch(headers: axum::http::HeaderMap) -> axum::response::Response {
    let base = base_url_from_headers(&headers);
    let xml = format!(
        r#"<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/" xmlns:moz="http://www.mozilla.org/2006/browser/search/">
  <ShortName>LinkList</ShortName>
  <Description>LinkList</Description>
  <InputEncoding>UTF-8</InputEncoding>
  <Image width="56" height="56" type="image/png">{base}/logo.png</Image>
  <Url type="text/html" template="{base}/bookmarks?client=opensearch&amp;q={{searchTerms}}"/>
</OpenSearchDescription>
"#
    );
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/opensearchdescription+xml",
        )],
        xml,
    )
        .into_response()
}

/// `GET /manifest.json`：PWA manifest（对齐上游 manifest 视图）。
/// 背景色随当前用户主题；匿名/自动主题用浅色。icons 用本地 logo.png。
async fn manifest(
    State(state): State<AppState>,
    session: tower_sessions::Session,
) -> axum::Json<serde_json::Value> {
    let viewer = crate::web::auth_api::current_user(&state.pool, &session)
        .await
        .ok()
        .flatten();
    let viewer_id = viewer.as_ref().map(|u| u.id);
    let theme = crate::web::profile_api::effective_profile(&state.pool, viewer_id)
        .await
        .theme;
    let background_color = if theme == "dark" {
        "#161822"
    } else {
        "#ffffff"
    };
    axum::Json(serde_json::json!({
        "short_name": "LinkList",
        "name": "LinkList",
        "description": "Self-hosted bookmark service",
        "start_url": "/bookmarks",
        "display": "standalone",
        "scope": "/",
        "theme_color": "#5856e0",
        "background_color": background_color,
        "icons": [
            {"src": "/logo-512.png", "type": "image/png", "sizes": "512x512", "purpose": "any"},
            {"src": "/icons/icon-192.png", "type": "image/png", "sizes": "192x192", "purpose": "any"},
            {"src": "/icons/icon-512.png", "type": "image/png", "sizes": "512x512", "purpose": "maskable"},
        ],
        "shortcuts": [
            {"name": "添加书签", "url": "/bookmarks/new"},
            {"name": "已归档", "url": "/bookmarks/archived"},
            {"name": "未读", "url": "/bookmarks?unread=yes"},
            {"name": "无标签", "url": "/bookmarks?q=!untagged"},
            {"name": "共享", "url": "/bookmarks/shared"},
        ],
        "share_target": {
            "action": "/bookmarks/new",
            "method": "GET",
            "enctype": "application/x-www-form-urlencoded",
            "params": {"url": "url", "text": "url", "title": "title"},
        },
    }))
}

/// `GET /custom_css`：返回生效资料的自定义 CSS
/// （登录=本人资料；匿名=访客资料 guest_profile_user；空 CSS 返回 204；命中时 text/css + 30 天缓存）。
async fn custom_css(
    State(state): State<AppState>,
    session: tower_sessions::Session,
) -> axum::response::Response {
    let viewer = crate::web::auth_api::current_user(&state.pool, &session)
        .await
        .ok()
        .flatten();
    let viewer_id = viewer.as_ref().map(|u| u.id);
    let profile = crate::web::profile_api::effective_profile(&state.pool, viewer_id).await;
    let css = profile.custom_css;
    if css.is_empty() {
        return axum::http::StatusCode::NO_CONTENT.into_response();
    }
    (
        axum::http::StatusCode::OK,
        [
            (axum::http::header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (axum::http::header::CACHE_CONTROL, "public, max-age=2592000"),
        ],
        css,
    )
        .into_response()
}

/// 由请求头构造绝对 base URL（优先 X-Forwarded-Proto；缺失 Host 时退化为空）。
pub(crate) fn base_url_from_headers(headers: &axum::http::HeaderMap) -> String {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    if host.is_empty() {
        return String::new();
    }
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}")
}
