use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

/// 嵌入 frontend/dist 构建产物（Astro 静态多页，`pnpm build` 后生成）。
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Assets;

/// 页面路由表：路径 → 产物内 HTML 文件（Astro 目录式输出 `dir/index.html`）。
fn page_for_path(path: &str) -> Option<&'static str> {
    match path {
        "/" | "/index.html" => Some("index.html"),
        "/login" => Some("login/index.html"),
        "/bookmarks" => Some("bookmarks/index.html"),
        "/bookmarks/new" => Some("bookmarks/new/index.html"),
        "/bookmarks/edit" => Some("bookmarks/edit/index.html"),
        "/bookmarks/archived" => Some("archived/index.html"),
        "/shared" => Some("public-shared/index.html"),
        "/bookmarks/shared" => Some("shared/index.html"),
        "/tags" => Some("tags/index.html"),
        "/bundles" => Some("bundles/index.html"),
        "/settings" => Some("settings/index.html"),
        "/settings/general" => Some("settings/general/index.html"),
        "/settings/integrations" => Some("settings/integrations/index.html"),
        "/admin" => Some("admin/index.html"),
        _ => None,
    }
}

/// `GET /` 与多页路由：返回嵌入的 HTML。
/// 服务端鉴权：受保护页面未登录直接 302 到 /login，不先暴露后台页面内容；
/// 已登录访问 /login 则跳回 /bookmarks。公开页（登录页、共享页）不检查。
pub async fn page(
    path: &str,
    axum::extract::State(state): axum::extract::State<crate::web::auth_api::AppState>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
) -> Response {
    // /bookmarks/shared 对齐 linkding shared 视图：无 @login_required，匿名可见（仅公开共享书签）。
    let public_page = matches!(path, "/" | "/login" | "/shared" | "/bookmarks/shared");
    if !public_page {
        match crate::web::auth_api::current_user(&state.pool, &session).await {
            Ok(Some(_)) => {}
            _ => return redirect(&headers, "/login"),
        }
    }
    // 根路径对齐 linkding root 视图：未登录且落地页为共享书签页时 302 到 /bookmarks/shared，
    // 已登录（或落地页为登录页）进书签列表。
    if path == "/" {
        let landing_shared =
            crate::db::q_scalar::<()>("SELECT value FROM settings WHERE key = 'landing_page'")
                .fetch_optional(&state.pool)
                .await
                .ok()
                .flatten()
                .and_then(|r| r.get_opt_string(0))
                .map(|v| v == "shared_bookmarks")
                .unwrap_or(false);
        let authed = matches!(
            crate::web::auth_api::current_user(&state.pool, &session).await,
            Ok(Some(_))
        );
        if !authed && landing_shared {
            return redirect(&headers, "/bookmarks/shared");
        }
        return redirect(&headers, "/bookmarks");
    }
    if path == "/login"
        && matches!(
            crate::web::auth_api::current_user(&state.pool, &session).await,
            Ok(Some(_))
        )
    {
        return redirect(&headers, "/bookmarks");
    }
    match page_for_path(path).and_then(Assets::get) {
        Some(asset) => {
            Html(String::from_utf8_lossy(asset.data.as_ref()).into_owned()).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn redirect(headers: &axum::http::HeaderMap, to: &str) -> Response {
    // Location 用绝对 URL（对齐 Django HttpResponseRedirect）：
    // 优先取请求 Host 头（浏览器实际访问的主机:端口），避免以 0.0.0.0 访问或
    // 反向代理场景下 Uri 提取器拿不到 Host 导致的地址错误；缺失时退化为相对路径。
    let location = match headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
    {
        Some(host) if !host.is_empty() => {
            let scheme = headers
                .get("x-forwarded-proto")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("http");
            format!("{scheme}://{host}{to}")
        }
        _ => to.to_string(),
    };
    (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
}

/// 页面路由便捷包装：带服务端会话鉴权的 `GET` handler。
pub fn page_route(
    path: &'static str,
) -> axum::routing::MethodRouter<crate::web::auth_api::AppState> {
    axum::routing::get(
        move |axum::extract::State(state): axum::extract::State<crate::web::auth_api::AppState>,
              session: tower_sessions::Session,
              headers: axum::http::HeaderMap| async move {
            page(path, axum::extract::State(state), session, headers).await
        },
    )
}

/// `GET /bundles/{id}/edit`：Bundle 独立编辑页（任意数字 id 均服务同一静态页，
/// 页面脚本从路径提取 id 并自行校验归属；对齐 linkding /bundles/{id}/edit）。
pub async fn bundles_edit_page(
    axum::extract::State(state): axum::extract::State<crate::web::auth_api::AppState>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
) -> Response {
    match crate::web::auth_api::current_user(&state.pool, &session).await {
        Ok(Some(_)) => {}
        _ => return redirect(&headers, "/login"),
    }
    match Assets::get("bundles/edit/index.html") {
        Some(asset) => {
            Html(String::from_utf8_lossy(asset.data.as_ref()).into_owned()).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /assets/{*path}`：静态资源（CSS/JS/图片，Astro 产物统一在 assets/）。
pub async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    serve_file(&format!("assets/{path}")).await
}

/// 根目录静态文件：支持单段（icons.svg、logo.png、sw.js、favicon.ico）
/// 与多段子路径（icons/icon-192.png 等）；仅服务嵌入产物内的静态文件。
pub async fn static_file(axum::extract::Path(file): axum::extract::Path<String>) -> Response {
    serve_file(&file).await
}

async fn serve_file(full: &str) -> Response {
    match Assets::get(full) {
        Some(asset) => {
            let mime = asset.metadata.mimetype();
            (
                [
                    (header::CONTENT_TYPE, mime.to_string()),
                    // 指纹文件长缓存；根目录文件短缓存。
                    (
                        header::CACHE_CONTROL,
                        if full.contains('.')
                            && full.rsplit('.').next().is_some_and(|e| e.len() >= 4)
                        {
                            "public, max-age=86400".to_string()
                        } else {
                            "public, max-age=3600".to_string()
                        },
                    ),
                ],
                asset.data,
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
