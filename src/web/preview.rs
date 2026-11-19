use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::error::AppError;
use crate::web::auth_api::AppState;

/// `GET /previews/{bookmark_id}`：按书签 ID 返回其预览图文件。
/// 文件名含内容哈希（sha256(url)+ext），同书签 URL 变更后路径不变但内容更新，
/// 故缓存头用短 max-age（对齐上游 static 服务行为）。
pub async fn preview_image(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let path: Option<String> =
        crate::db::q_scalar::<()>("SELECT preview_image_path FROM bookmarks WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .and_then(|r| r.get_opt_string(0));
    let Some(file_name) = path.filter(|p| !p.is_empty()) else {
        return Err(AppError::NotFound);
    };
    // 文件名仅允许 hex + 已知扩展，防路径穿越。
    if !file_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.')
        || file_name.contains("..")
    {
        return Err(AppError::BadRequest("无效预览图路径".into()));
    }
    let full = crate::capture::preview::previews_dir(&state.config.data_dir).join(&file_name);
    let bytes = tokio::fs::read(&full)
        .await
        .map_err(|_| AppError::NotFound)?;
    let mime = crate::capture::preview::content_type_for_file_name(&file_name);
    Ok((
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CACHE_CONTROL, "public, max-age=3600".to_string()),
        ],
        bytes,
    )
        .into_response())
}
