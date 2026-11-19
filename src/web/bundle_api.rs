use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::bookmarks::bundle::{self, BundleSearch, NewBundle};
use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};
use axum::extract::Query as AxumQuery;
use serde::Deserialize;

/// 预览查询参数（对齐 linkding bundles.preview：表单字段经 GET 参数实时构造过滤器）。
#[derive(Deserialize, Default)]
pub struct PreviewQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    /// 搜索词（所有词都必须命中）。
    pub search: Option<String>,
    /// 标签（任一命中，逗号分隔）。
    pub any_tags: Option<String>,
    /// 必选标签（全部命中，逗号分隔）。
    pub all_tags: Option<String>,
    /// 排除标签（任一命中即排除，逗号分隔）。
    pub excluded_tags: Option<String>,
    /// 阅读状态：yes=未读，no=已读，off/缺省=全部。
    pub filter_unread: Option<String>,
    /// 共享状态：yes=已共享，no=未共享，off/缺省=全部。
    pub filter_shared: Option<String>,
}

fn split_tags(v: &Option<String>) -> Vec<String> {
    v.as_deref()
        .map(|s| {
            s.split(|c: char| c == ',' || c == '，' || c.is_whitespace())
                .map(|t| t.trim().trim_start_matches('#'))
                .filter(|t| !t.is_empty())
                .map(|t| t.to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn filter_state(v: &Option<String>) -> Option<bool> {
    match v.as_deref().unwrap_or("off") {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// 按预览参数构造 ListParams（对齐 linkding _get_bookmark_list_preview 的表单实时构造）。
fn preview_list_params(q: &PreviewQuery) -> crate::bookmarks::crud::ListParams {
    let search = q.search.as_deref().unwrap_or("").trim();
    let expr = if search.is_empty() {
        None
    } else {
        crate::bookmarks::search::parse_query(search).ok()
    };
    crate::bookmarks::crud::ListParams {
        filter: Some("active".into()),
        tags_and: split_tags(&q.all_tags),
        tags_or: split_tags(&q.any_tags),
        tags_not: split_tags(&q.excluded_tags),
        shared: filter_state(&q.filter_shared),
        unread: filter_state(&q.filter_unread),
        q: if expr.is_none() && !search.is_empty() {
            Some(search.to_string())
        } else {
            None
        },
        expr,
        lax_tag_search: false,
        sort: None,
        page: q.page.unwrap_or(1).max(1),
        modified_since: None,
        added_since: None,
        per_page: q.per_page.unwrap_or(30).clamp(1, 200),
    }
}

/// `GET /api/v1/bundles/preview`：按表单参数实时预览过滤器命中结果。
/// 注意路由需注册在 `/api/v1/bundles/{id}` 之前，避免 "preview" 被当作 id。
pub async fn preview(
    State(state): State<AppState>,
    auth: AuthUser,
    AxumQuery(q): AxumQuery<PreviewQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let lp = preview_list_params(&q);
    let (items, total) = crate::bookmarks::crud::list(&state.pool, auth.0.id, &lp).await?;
    let mut results = Vec::with_capacity(items.len());
    for b in &items {
        let mut v = serde_json::to_value(b).map_err(AppError::internal)?;
        v["tags"] =
            serde_json::json!(crate::bookmarks::tags::tags_for_bookmark(&state.pool, b.id).await?);
        results.push(v);
    }
    Ok(Json(serde_json::json!({
        "total": total,
        "page": lp.page,
        "per_page": lp.effective_per_page(),
        "results": results,
    })))
}

/// `GET /api/v1/bundles/preview/tags`：预览条件下书签使用的标签（暂未用于 UI，保持接口完备）。
pub async fn preview_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    AxumQuery(q): AxumQuery<PreviewQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let lp = preview_list_params(&q);
    let items = crate::bookmarks::crud::list_tags_for_query(&state.pool, auth.0.id, &lp).await?;
    let mut tag_items: Vec<crate::bookmarks::tags::TagWithCount> = items
        .into_iter()
        .map(|(name, cnt)| crate::bookmarks::tags::TagWithCount {
            id: 0,
            name,
            usage_count: cnt,
        })
        .collect();
    crate::bookmarks::tags::sort_tags_english_first(&mut tag_items);
    Ok(Json(serde_json::json!({ "results": tag_items })))
}

/// `GET /api/v1/bundles`：列出当前用户全部保存过滤器。
pub async fn list_bundles(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let items = bundle::list(&state.pool, auth.0.id).await?;
    Ok(Json(serde_json::json!({"results": items})))
}

/// `POST /api/v1/bundles/reorder`：按传入 id 顺序重排（对齐 linkding 拖拽）。
pub async fn reorder_bundles(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let ids: Vec<i64> = req
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
        .ok_or_else(|| AppError::BadRequest("ids 字段无效".into()))?;
    bundle::reorder(&state.pool, auth.0.id, &ids).await?;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// `POST /api/v1/bundles`：创建保存过滤器。
pub async fn create_bundle(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<NewBundle>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let b = bundle::create(&state.pool, auth.0.id, &req).await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::to_value(&b).map_err(AppError::internal)?),
    ))
}

/// `GET /api/v1/bundles/{id}`：读取单个。
pub async fn get_bundle(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let b = bundle::get(&state.pool, auth.0.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::to_value(&b).map_err(AppError::internal)?))
}

/// `PATCH /api/v1/bundles/{id}`：更新名称/条件。
pub async fn update_bundle(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let search: Option<BundleSearch> = match req.get("search").or_else(|| req.get("search_json")) {
        Some(v) if !v.is_null() => Some(
            serde_json::from_value(v.clone())
                .map_err(|e| AppError::BadRequest(format!("search 字段无效: {e}")))?,
        ),
        _ => None,
    };
    let b = bundle::update(&state.pool, auth.0.id, id, name, search)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::to_value(&b).map_err(AppError::internal)?))
}

/// `DELETE /api/v1/bundles/{id}`：删除（不影响书签）。
pub async fn delete_bundle(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let deleted = bundle::delete(&state.pool, auth.0.id, id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppError::NotFound)
    }
}

/// `GET /api/v1/bundles/{id}/bookmarks`：应用过滤器查询书签。
pub async fn bundle_bookmarks(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let b = bundle::get(&state.pool, auth.0.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut params = crate::bookmarks::crud::ListParams::default();
    bundle::apply_to_params(&b, &mut params)?;
    let (items, total) = crate::bookmarks::crud::list(&state.pool, auth.0.id, &params).await?;
    Ok(Json(serde_json::json!({"total": total, "results": items})))
}
