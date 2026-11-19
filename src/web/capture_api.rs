use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::error::AppError;
use crate::tasks::queue::KIND_METADATA;
use crate::web::auth_api::{AppState, AuthUser};

/// `GET /api/v1/bookmarks/{id}/snapshot`：流式返回快照 HTML；无快照 404。
/// 对齐上游 assets.view：Content-Disposition inline + sandbox CSP。
pub async fn get_snapshot(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let row = crate::db::q("SELECT snapshot_path FROM bookmarks WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(auth.0.id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;
    let path: Option<String> = row.get_named_opt_string("snapshot_path");
    let path = path.ok_or(AppError::NotFound)?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?; // 文件缺失按 404。
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    let mut resp = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_static("inline; filename=\"snapshot.html\""),
    );
    if let Ok(name) = header::HeaderName::from_lowercase(b"content-security-policy") {
        resp.headers_mut().insert(
            name,
            header::HeaderValue::from_static("sandbox allow-scripts"),
        );
    }
    Ok(resp)
}

/// 上传快照大小上限：50 MiB（覆盖大多数单文件页）。
const MAX_SNAPSHOT_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// `POST /api/v1/bookmarks/{id}/singlefile`：浏览器端上传 single-file 快照。
///
/// 对齐上游 `POST api/bookmarks/singlefile/`（routes.py:136-166）：
/// - body 为 single-file 生成的完整 HTML（可 gzip，`Content-Encoding: gzip` 或魔数 1f 8b 自动解压）；
/// - 写入快照目录并回填 `bookmarks.snapshot_path`，前端 `has_snapshot` 随之生效。
pub async fn upload_singlefile(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    // 对齐上游 LD_DISABLE_ASSET_UPLOAD：关闭后浏览器端快照上传 403。
    if state.config.disable_asset_upload {
        return Err(AppError::Forbidden);
    }
    // 权限：仅本人书签。
    let exists =
        crate::db::q_scalar::<i64>("SELECT id FROM bookmarks WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(auth.0.id)
            .fetch_optional(&state.pool)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    if body.is_empty() {
        return Err(AppError::BadRequest("上传内容为空".into()));
    }
    if body.len() > MAX_SNAPSHOT_UPLOAD_BYTES {
        return Err(AppError::BadRequest(format!(
            "快照超过大小上限 {MAX_SNAPSHOT_UPLOAD_BYTES} 字节"
        )));
    }

    // gzip 透传：按 Content-Encoding 或魔数解压（上游 upload_snapshot 同语义）。
    let is_gzip_encoding = headers
        .get(axum::http::header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("gzip"))
        .unwrap_or(false);
    let looks_gzip = body.len() >= 2 && body[0] == 0x1f && body[1] == 0x8b;
    let html: Vec<u8> = if is_gzip_encoding || looks_gzip {
        let mut decoder = flate2::read::GzDecoder::new(&body[..]);
        let mut out = Vec::with_capacity(body.len() * 4);
        std::io::Read::read_to_end(&mut decoder, &mut out)
            .map_err(|e| AppError::BadRequest(format!("gzip 解压失败: {e}")))?;
        out
    } else {
        body.to_vec()
    };

    // 内容必须是 HTML（对齐上游 upload_snapshot 校验）。
    let text = String::from_utf8_lossy(&html);
    let lowered = text.to_ascii_lowercase();
    if !lowered.contains("<html") && !lowered.contains("<!doctype html") {
        return Err(AppError::BadRequest("内容不是有效的 HTML 快照".into()));
    }

    // 落盘：{data_dir}/snapshots/{bookmark_id}.html（与服务端抓取同一位置）。
    let path = crate::capture::snapshot::snapshot_path(&state.config.data_dir, id);
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(AppError::internal)?;
    }
    tokio::fs::write(&path, &html)
        .await
        .map_err(AppError::internal)?;

    // 回填快照路径与时间。
    let now = chrono::Utc::now().to_rfc3339();
    crate::db::q("UPDATE bookmarks SET snapshot_path = ?, snapshot_date = ?, updated_at = ? WHERE id = ? AND user_id = ?")
        .bind(path.to_string_lossy().as_ref())
        .bind(&now)
        .bind(&now)
        .bind(id)
        .bind(auth.0.id)
        .execute(&state.pool)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "ok": true,
            "bookmark_id": id,
            "size": html.len(),
            "gzip": is_gzip_encoding || looks_gzip,
        })),
    ))
}
/// `POST /api/v1/bookmarks/{id}/capture`：手动触发抓取任务（metadata/snapshot/archive_lookup）。
pub async fn trigger_capture(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<TriggerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    // 权限：仅本人书签。
    let exists =
        crate::db::q_scalar::<i64>("SELECT id FROM bookmarks WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(auth.0.id)
            .fetch_optional(&state.pool)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }
    // 对齐上游：快照任务受 LD_ENABLE_SNAPSHOTS 门控；全部任务受 LD_DISABLE_BACKGROUND_TASKS 门控。
    if state.config.disable_background_tasks {
        return Err(AppError::BadRequest(
            "后台任务已禁用（disable_background_tasks）".into(),
        ));
    }
    if req.kind == crate::tasks::queue::KIND_SNAPSHOT && !state.config.enable_snapshots {
        return Err(AppError::BadRequest(
            "快照功能未启用（enable_snapshots）".into(),
        ));
    }
    let queue = state
        .task_queue
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("任务队列未启用".into()))?;
    let task_id = queue.enqueue(id, &req.kind).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({"task_id": task_id, "kind": req.kind})),
    ))
}

/// `POST /api/v1/capture-tasks/{task_id}/retry`：重试失败任务（仅本人书签的任务）。
pub async fn retry_task(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(task_id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = crate::db::q(
        "SELECT t.id, t.status, t.bookmark_id FROM capture_tasks t
         JOIN bookmarks b ON b.id = t.bookmark_id
         WHERE t.id = ? AND b.user_id = ?",
    )
    .bind(task_id)
    .bind(auth.0.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let status: String = row.get_named_string("status");
    if status != "failed" {
        return Err(AppError::BadRequest(format!(
            "任务状态为 {status}，仅失败任务可重试"
        )));
    }
    crate::db::q("UPDATE capture_tasks SET status='pending', attempts=0, last_error=NULL, updated_at=? WHERE id=?")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(task_id)
        .execute(&state.pool)
        .await?;
    if let Some(queue) = &state.task_queue {
        queue.requeue(task_id).await;
    }
    Ok(Json(serde_json::json!({"ok": true, "task_id": task_id})))
}

/// `GET /api/v1/bookmarks/{id}/capture-tasks`：查看书签的任务状态。
pub async fn list_tasks(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows = crate::db::q(
        "SELECT t.id, t.kind, t.status, t.attempts, t.last_error, t.created_at, t.updated_at
         FROM capture_tasks t JOIN bookmarks b ON b.id = t.bookmark_id
         WHERE t.bookmark_id = ? AND b.user_id = ? ORDER BY t.id DESC",
    )
    .bind(id)
    .bind(auth.0.id)
    .fetch_all(&state.pool)
    .await?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get_named_i64("id"),
                "kind": r.get_named_string("kind"),
                "status": r.get_named_string("status"),
                "attempts": r.get_u32_named("attempts"),
                "last_error": r.get_named_opt_string("last_error"),
                "created_at": r.get_named_string("created_at"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"results": items})))
}

#[derive(Deserialize)]
pub struct TriggerRequest {
    /// metadata / snapshot / archive_lookup；缺省 metadata。
    #[serde(default = "default_kind")]
    pub kind: String,
}

fn default_kind() -> String {
    KIND_METADATA.to_string()
}

#[cfg(test)]
mod tests {
    use crate::web::auth_api::integration::{get, login_cookie, post_json, post_with_cookie};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// TODO2 回归：singlefile 上传 → 快照可读、gzip 透传解压、越权/未认证拒绝。
    #[tokio::test]
    async fn singlefile_upload_roundtrip() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config {
            data_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let app =
            crate::web::auth_api::integration::build_app_with_config(pool.clone(), None, cfg).await;

        let cookie = login_cookie(&app).await;
        let (s, b, _) = post_with_cookie(
            &app,
            "/api/v1/bookmarks",
            r#"{"url":"https://single.example/","title":"Single"}"#,
            Some(&cookie),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
        let bid = b["id"].as_i64().unwrap();

        // 1. 明文上传 → 201，快照可读。
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/bookmarks/{bid}/singlefile"))
            .header("cookie", &cookie)
            .header("content-type", "text/html")
            .body(Body::from("<html><body>SNAPSHOT</body></html>"))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let (s, _) = get(
            &app,
            &format!("/api/v1/bookmarks/{bid}/snapshot"),
            Some(&cookie),
        )
        .await;
        assert_eq!(s, StatusCode::OK);

        // 2. gzip 上传 → 透传解压成功。
        use std::io::Write as _;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(b"<!DOCTYPE html><html>GZIPPED</html>")
            .unwrap();
        let gz = enc.finish().unwrap();
        let req = Request::builder()
            .method("POST")
            .uri(format!("/api/v1/bookmarks/{bid}/singlefile"))
            .header("cookie", &cookie)
            .header("content-type", "text/html")
            .header("content-encoding", "gzip")
            .body(Body::from(gz))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // 3. 非 HTML 内容 → 400。
        let (s, _, _) = post_with_cookie(
            &app,
            &format!("/api/v1/bookmarks/{bid}/singlefile"),
            "not html",
            Some(&cookie),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // 4. 他人书签不可上传 → 404。
        crate::models::create_user(&pool, "alice", "password123", false)
            .await
            .unwrap();
        let (_, _, set_cookie2) = post_json(
            &app,
            "/api/v1/auth/login",
            r#"{"username":"alice","password":"password123"}"#,
        )
        .await;
        let cookie2 = set_cookie2
            .map(|c| c.split(';').next().unwrap().to_string())
            .unwrap();
        let (s, _, _) = post_with_cookie(
            &app,
            &format!("/api/v1/bookmarks/{bid}/singlefile"),
            "<html>x</html>",
            Some(&cookie2),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);

        // 5. 未认证快照读取 → 401。
        let (s, _) = get(&app, &format!("/api/v1/bookmarks/{bid}/snapshot"), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
}
