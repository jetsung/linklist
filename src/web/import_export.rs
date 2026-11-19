use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::Response;

use crate::bookmarks::netscape;
use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};

use axum::extract::Query;
use serde::Deserialize;

/// 导入查询参数（对齐上游 settings bookmark_import 的 map_private_flag）。
#[derive(Debug, Default, Deserialize)]
pub struct ImportQuery {
    /// "true"/"on" 时启用 PRIVATE→共享 映射开关。
    pub map_private_flag: Option<String>,
}

/// `POST /api/v1/bookmarks/import?map_private_flag=true`：Netscape HTML 导入。
pub async fn import(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ImportQuery>,
    body: String,
) -> Result<Json<serde_json::Value>, AppError> {
    if body.trim().is_empty() {
        return Err(AppError::BadRequest("导入内容为空".into()));
    }
    let options = netscape::ImportOptions {
        map_private_flag: params
            .map_private_flag
            .as_deref()
            .map(|v| v == "true" || v == "on")
            .unwrap_or(false),
    };
    let stats = netscape::import(&state.pool, auth.0.id, &body, &options).await?;
    Ok(Json(
        serde_json::to_value(&stats).map_err(AppError::internal)?,
    ))
}

/// `GET /api/v1/bookmarks/export`：Netscape HTML 导出。
/// 对齐 linkding settings.export：以 attachment 方式下载，文件名 bookmarks_年-月-日_时-分-秒.html。
pub async fn export(State(state): State<AppState>, auth: AuthUser) -> Result<Response, AppError> {
    let html = netscape::export(&state.pool, auth.0.id).await?;
    let now = chrono::Utc::now();
    let filename = format!("bookmarks_{}.html", now.format("%Y-%m-%d_%H-%M-%S"));
    let mut resp = Response::new(html.into());
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    if let Ok(v) = header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use crate::web::auth_api::integration::{
        build_app_with_pool, get_raw_with_cookie, post_json, post_with_cookie,
    };

    fn cookie_value(set_cookie: &Option<String>) -> Option<String> {
        set_cookie
            .as_ref()
            .map(|c| c.split(';').next().unwrap_or("").to_string())
    }

    /// TODO1 回归：导出必须包含已归档书签，并带 ADD_DATE/LAST_MODIFIED/PRIVATE/TOREAD、
    /// 归档伪标签与 [linkding-notes]（对齐 linkding exporter）。
    #[tokio::test]
    async fn export_includes_archived_and_attributes() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let app = build_app_with_pool(pool.clone(), None).await.0;

        // setup 拿 cookie。
        let (_, _, set_cookie) = post_json(
            &app,
            "/api/v1/auth/setup",
            r#"{"username":"admin","password":"password123"}"#,
        )
        .await;
        let cookie = cookie_value(&set_cookie).unwrap();

        // 建 3 条书签：未读+共享、稍后归档、带 notes。
        let (s, _, _) = post_with_cookie(
            &app,
            "/api/v1/bookmarks",
            r#"{"url":"https://a.com/","title":"A","is_unread":true,"is_shared":true}"#,
            Some(&cookie),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::CREATED);
        let (s, b, _) = post_with_cookie(
            &app,
            "/api/v1/bookmarks",
            r#"{"url":"https://archived.com/","title":"Archived"}"#,
            Some(&cookie),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::CREATED);
        let archived_id = b["id"].as_i64().unwrap();
        let (s, _, _) = post_with_cookie(
            &app,
            "/api/v1/bookmarks",
            r#"{"url":"https://notes.com/","title":"Notes","description":"desc","notes":"hello note"}"#,
            Some(&cookie),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::CREATED);

        // 归档第二条。
        let (s, _, _) = post_with_cookie(
            &app,
            &format!("/api/v1/bookmarks/{archived_id}/archive"),
            "",
            Some(&cookie),
        )
        .await;
        assert_eq!(s, axum::http::StatusCode::OK);

        // 导出（原始文本）。
        let (status, body) =
            get_raw_with_cookie(&app, "/api/v1/bookmarks/export", Some(&cookie)).await;
        assert_eq!(status, axum::http::StatusCode::OK);

        // 已归档书签必须出现在导出中，并带归档伪标签。
        assert!(body.contains("https://archived.com/"), "已归档书签应被导出");
        assert!(
            body.contains("linkding:bookmarks.archived"),
            "归档伪标签应存在"
        );
        // 属性齐全。
        assert!(body.contains("ADD_DATE=\""), "应有 ADD_DATE");
        assert!(body.contains("LAST_MODIFIED=\""), "应有 LAST_MODIFIED");
        assert!(body.contains("PRIVATE=\"0\""), "共享书签应 PRIVATE=0");
        assert!(body.contains("TOREAD=\"1\""), "未读书签应 TOREAD=1");
        assert!(body.contains("TAGS=\""), "应有 TAGS");
        // notes 以 [linkding-notes] 形式输出。
        assert!(
            body.contains("[linkding-notes]hello note[/linkding-notes]"),
            "notes 应输出到 DD"
        );
        assert!(body.contains("desc"), "描述应保留");
    }
}
