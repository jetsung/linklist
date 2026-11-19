use std::path::PathBuf;

use axum::Json;
use axum::extract::{Multipart, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};

/// 上传大小上限：10 MiB。
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;

/// 允许的上传类型白名单。
fn allowed_content_type(ct: &str) -> bool {
    let ct = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    matches!(
        ct.as_str(),
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/svg+xml"
            | "application/pdf"
            | "text/plain"
            | "text/html"
            | "application/zip"
    )
}

fn assets_dir(data_dir: &std::path::Path) -> PathBuf {
    data_dir.join("assets")
}

/// `POST /api/v1/bookmarks/{id}/assets`：multipart 上传资产。
pub async fn upload_asset(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    // 对齐上游 LD_DISABLE_ASSET_UPLOAD：关闭后资产上传 403。
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

    let mut file_name = String::from("upload.bin");
    let mut content_type = String::from("application/octet-stream");
    let mut data: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart 解析失败: {e}")))?
    {
        if field.name().unwrap_or("") == "file" {
            if let Some(fname) = field.file_name() {
                file_name = fname.to_string();
            }
            if let Some(ct) = field.content_type() {
                content_type = ct.to_string();
            }
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("读取上传内容失败: {e}")))?;
            data = Some(bytes.to_vec());
        }
    }
    let data = data.ok_or_else(|| AppError::BadRequest("缺少 file 字段".into()))?;

    // 类型与大小校验。
    if !allowed_content_type(&content_type) {
        return Err(AppError::BadRequest(format!(
            "不允许的内容类型: {content_type}"
        )));
    }
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(AppError::BadRequest(format!(
            "文件超过大小上限 {MAX_UPLOAD_BYTES} 字节"
        )));
    }

    // 存储：{data_dir}/assets/{bookmark_id}/{uuid}_{filename}。
    let dir = assets_dir(&state.config.data_dir).join(id.to_string());
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(AppError::internal)?;
    let safe_name: String = file_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let unique = format!(
        "{}_{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
        safe_name
    );
    let file_path = dir.join(&unique);
    tokio::fs::write(&file_path, &data)
        .await
        .map_err(AppError::internal)?;

    let now = chrono::Utc::now().to_rfc3339();
    let result = crate::db::q(
        "INSERT INTO bookmark_assets (bookmark_id, asset_type, file_path, file_size, content_type, display_name, status, date_created)
         VALUES (?, 'upload', ?, ?, ?, ?, 'complete', ?)",
    )
    .bind(id)
    .bind(file_path.to_string_lossy().as_ref())
    .bind(data.len() as i64)
    .bind(&content_type)
    .bind(&file_name)
    .bind(&now)
    .execute(&state.pool)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": result.last_insert_rowid(),
            "bookmark_id": id,
            "file_name": file_name,
            "file_size": data.len(),
            "content_type": content_type,
        })),
    ))
}

/// `GET /api/v1/bookmarks/{id}/assets`：列出资产（不含文件内容）。
pub async fn list_assets(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let rows = crate::db::q(
        "SELECT a.id, a.asset_type, a.file_size, a.content_type, a.display_name, a.date_created
         FROM bookmark_assets a JOIN bookmarks b ON b.id = a.bookmark_id
         WHERE a.bookmark_id = ? AND b.user_id = ? ORDER BY a.id",
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
                "asset_type": r.get_named_string("asset_type"),
                "file_size": r.get_named_i64("file_size"),
                "content_type": r.get_named_string("content_type"),
                "display_name": r.get_named_string("display_name"),
                "date_created": r.get_named_string("date_created"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"results": items})))
}

/// 按上游 assets.view 语义生成安全头：Content-Disposition inline + 按 content_type 的 CSP。
fn asset_security_headers(content_type: &str, filename: &str) -> Vec<(header::HeaderName, String)> {
    let mut headers = vec![(
        header::CONTENT_DISPOSITION,
        format!("inline; filename=\"{}\"", filename.replace('"', "")),
    )];
    let ct = content_type.split(';').next().unwrap_or("").trim();
    let csp = if ct.starts_with("video/") {
        "default-src 'none'; media-src 'self';"
    } else if ct == "application/pdf" {
        "default-src 'none'; object-src 'self';"
    } else {
        "sandbox allow-scripts"
    };
    if let Ok(name) = header::HeaderName::from_lowercase(b"content-security-policy") {
        headers.push((name, csp.to_string()));
    }
    headers
}

/// 快照 download_name：PDF → .pdf，其余 → .html（对齐上游 BookmarkAsset.download_name）。
fn snapshot_download_name(content_type: &str) -> String {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    if ct == "application/pdf" {
        "snapshot.pdf".into()
    } else {
        "snapshot.html".into()
    }
}

/// `GET /api/v1/bookmarks/{id}/assets/{aid}`：内联查看（Content-Disposition inline + CSP）。
pub async fn download_asset(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, aid)): Path<(i64, i64)>,
) -> Result<Response, AppError> {
    let row = crate::db::q(
        "SELECT a.file_path, a.content_type, a.display_name, a.asset_type FROM bookmark_assets a
         JOIN bookmarks b ON b.id = a.bookmark_id
         WHERE a.id = ? AND a.bookmark_id = ? AND b.user_id = ?",
    )
    .bind(aid)
    .bind(id)
    .bind(auth.0.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let path: String = row.get_named_string("file_path");
    let content_type: String = row.get_named_string("content_type");
    let display_name: String = row.try_get_string_or("display_name", "asset");
    let asset_type: String = row.try_get_string_or("asset_type", "upload");
    // download_name（对齐上游）：snapshot 类型按 content_type 补扩展名，其余用 display_name。
    let filename = if asset_type == "snapshot" {
        snapshot_download_name(&content_type)
    } else {
        display_name
    };
    // 流式传输：不整读进内存（对齐上游 #1482）。文件缺失按 404。
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let stream = tokio_util::io::ReaderStream::new(file);
    let body = axum::body::Body::from_stream(stream);
    let mut resp = ([(header::CONTENT_TYPE, content_type.as_str())], body).into_response();
    for (name, value) in asset_security_headers(&content_type, &filename) {
        if let Ok(v) = header::HeaderValue::from_str(&value) {
            resp.headers_mut().insert(name, v);
        }
    }
    Ok(resp)
}

/// `GET /api/v1/bookmarks/{id}/assets/{aid}/read`：HTML 阅读页（对齐上游 assets.read）。
/// 内容嵌入 sandbox 页面（CSP: sandbox allow-scripts）。
pub async fn read_asset(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, aid)): Path<(i64, i64)>,
) -> Result<Response, AppError> {
    let row = crate::db::q(
        "SELECT a.file_path, a.content_type FROM bookmark_assets a
         JOIN bookmarks b ON b.id = a.bookmark_id
         WHERE a.id = ? AND a.bookmark_id = ? AND b.user_id = ?",
    )
    .bind(aid)
    .bind(id)
    .bind(auth.0.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let path: String = row.get_named_string("file_path");
    let content_type: String = row.get_named_string("content_type");
    let ct = content_type.split(';').next().unwrap_or("").trim();
    if ct != "text/html" {
        return Err(AppError::BadRequest("仅 HTML 资产支持阅读页".into()));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let content = String::from_utf8_lossy(&bytes);
    let page = format!(
        "<!DOCTYPE html><html lang=\"en\" class=\"reader-mode\"><head><meta charset=\"UTF-8\">\
<title>Reader view</title><meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0, minimal-ui\">\
<style>body{{margin:0 auto;max-width:720px;padding:16px;font-family:system-ui,sans-serif;line-height:1.6}}</style>\
</head><body>{}</body></html>",
        content
    );
    let mut resp = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response();
    if let Ok(name) = header::HeaderName::from_lowercase(b"content-security-policy") {
        resp.headers_mut().insert(
            name,
            header::HeaderValue::from_static("sandbox allow-scripts"),
        );
    }
    Ok(resp)
}

/// `DELETE /api/v1/bookmarks/{id}/assets/{aid}`：删除文件与记录。
pub async fn delete_asset(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((id, aid)): Path<(i64, i64)>,
) -> Result<StatusCode, AppError> {
    let row = crate::db::q(
        "SELECT a.file_path FROM bookmark_assets a
         JOIN bookmarks b ON b.id = a.bookmark_id
         WHERE a.id = ? AND a.bookmark_id = ? AND b.user_id = ?",
    )
    .bind(aid)
    .bind(id)
    .bind(auth.0.id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    let path: String = row.get_named_string("file_path");
    crate::db::q("DELETE FROM bookmark_assets WHERE id = ?")
        .bind(aid)
        .execute(&state.pool)
        .await?;
    // 删文件（尽力而为）。
    let _ = tokio::fs::remove_file(&path).await;
    Ok(StatusCode::NO_CONTENT)
}
