use crate::db::DbPool;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::bookmarks::crud::{self, ListParams, NewBookmark, UpdateBookmark};
use crate::bookmarks::tags;
use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};

/// `GET /api/v1/bookmarks`：分页列表。
pub async fn list(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Bundle（保存的过滤器）：命中时以其保存的搜索条件为基础，URL 显式参数可覆盖。
    let mut bundle_search = crate::bookmarks::bundle::BundleSearch::default();
    if let Some(bid) = params.bundle {
        match crate::bookmarks::bundle::get(&state.pool, auth.0.id, bid).await? {
            Some(b) => {
                bundle_search = serde_json::from_str(&b.search_json).map_err(AppError::internal)?;
            }
            None => return Err(AppError::NotFound),
        }
    }
    let q = params.q.or(bundle_search.q.clone());
    let filter = params.filter.or(bundle_search.filter.clone());
    // 标签搜索模式（对齐上游 profile.tag_search）。
    let lax_tag_search =
        crate::db::q_scalar::<String>("SELECT tag_search FROM user_profiles WHERE user_id = ?")
            .bind(auth.0.id)
            .fetch_optional(&state.pool)
            .await?
            .map(|r| r.get_string(0) == "lax")
            .unwrap_or(false);
    // 搜索表达式：q 传给解析器；解析失败 → 空结果（对齐上游 query_set.none()）。
    let expr = match q.as_deref() {
        Some(q) if !q.trim().is_empty() => match crate::bookmarks::search::parse_query(q) {
            Ok(e) => Some(e),
            Err(_) => {
                return Ok(Json(json!({
                    "count": 0, "total": 0,
                    "page": params.page.unwrap_or(1).max(1),
                    "per_page": params.per_page.unwrap_or(30).max(1),
                    "results": [],
                })));
            }
        },
        _ => None,
    };
    let lp = ListParams {
        filter,
        tags_and: params
            .tag
            .map(|t| vec![t])
            .unwrap_or_else(|| bundle_search.tags_and.clone()),
        tags_or: if params.tags_any.is_some() {
            params
                .tags_any
                .map(|s| {
                    s.split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            bundle_search.tags_or.clone()
        },
        tags_not: bundle_search.tags_not.clone(),
        shared: match params.shared.as_deref() {
            Some("yes") => Some(true),
            Some("no") => Some(false),
            _ => bundle_search.shared,
        },
        unread: match params.unread.as_deref() {
            Some("yes") => Some(true),
            Some("no") => Some(false),
            _ => None,
        },
        q,
        expr,
        lax_tag_search,
        sort: params.sort,
        page: params.page.unwrap_or(1),
        // 未显式传 per_page 时由下方用户偏好覆盖。
        modified_since: params.modified_since.clone(),
        added_since: params.added_since.clone(),
        per_page: params.per_page.unwrap_or(30).max(1),
    };
    let lp = if params.per_page.is_none() {
        let pref: Option<i64> =
            crate::db::q_scalar::<()>("SELECT items_per_page FROM user_profiles WHERE user_id = ?")
                .bind(auth.0.id)
                .fetch_optional(&state.pool)
                .await?
                .map(|r| r.get_i64(0));
        ListParams {
            per_page: pref.unwrap_or(30).max(1),
            ..lp
        }
    } else {
        lp
    };
    let (items, total) = crud::list(&state.pool, auth.0.id, &lp).await?;
    let mut results = Vec::with_capacity(items.len());
    for b in &items {
        let mut v = serde_json::to_value(b).map_err(AppError::internal)?;
        // has_snapshot：共享页日期链接需要（对齐上游 snapshot_url / shared_api 同名字段）。
        v["has_snapshot"] = json!(b.snapshot_path.is_some());
        // has_preview_image：前端预览图渲染开关字段（对齐上游 preview_image_file → static URL）。
        v["has_preview_image"] = json!(
            b.preview_image_path
                .as_deref()
                .is_some_and(|p| !p.is_empty())
        );
        v["tags"] = json!(tags::tags_for_bookmark(&state.pool, b.id).await?);
        results.push(v);
    }
    Ok(Json(json!({
        "count": results.len(),
        "total": total,
        "page": lp.page.max(1),
        "per_page": lp.effective_per_page(),
        "results": results,
    })))
}

/// `GET /api/v1/bookmarks/:id`
pub async fn get_one(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let b = crud::get(&state.pool, auth.0.id, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut v = serde_json::to_value(&b).map_err(AppError::internal)?;
    v["has_preview_image"] = json!(
        b.preview_image_path
            .as_deref()
            .is_some_and(|p| !p.is_empty())
    );
    v["tags"] = json!(tags::tags_for_bookmark(&state.pool, b.id).await?);
    Ok(Json(v))
}

/// `POST /api/v1/bookmarks`
pub async fn create(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<NewBookmark>,
) -> Result<Response, AppError> {
    if req.url.trim().is_empty() {
        return Err(AppError::BadRequest("URL 不能为空".into()));
    }
    if !state.config.disable_url_validation
        && let Err(msg) = crate::bookmarks::normalize::validate_bookmark_url(&req.url)
    {
        return Err(AppError::BadRequest(msg));
    }
    let b = crud::create(&state.pool, auth.0.id, &req).await?;
    // 入队抓取任务（尽力而为）：metadata 必入队；IA 存档查询按 web_archive_integration 偏好门控。
    // disable_background_tasks=true 时全部跳过（对齐上游 LD_DISABLE_BACKGROUND_TASKS）。
    if let Some(queue) = state
        .task_queue
        .as_ref()
        .filter(|_| !state.config.disable_background_tasks)
    {
        if let Err(e) = queue
            .enqueue(b.id, crate::tasks::queue::KIND_METADATA)
            .await
        {
            tracing::warn!("入队 metadata 任务失败: {e}");
        }
        // 自动创建快照（后台"自动创建快照"偏好 + 服务端快照能力开关，对齐上游
        // enable_automatic_html_snapshots + is_html_snapshot_feature_active）。
        if state.config.enable_snapshots
            && automatic_snapshots_enabled(&state.pool, auth.0.id).await
            && let Err(e) = queue
                .enqueue(b.id, crate::tasks::queue::KIND_SNAPSHOT)
                .await
        {
            tracing::warn!("入队 snapshot 任务失败: {e}");
        }
        if web_archive_integration_enabled(&state.pool, auth.0.id).await
            && let Err(e) = queue
                .enqueue(b.id, crate::tasks::queue::KIND_ARCHIVE_LOOKUP)
                .await
        {
            tracing::warn!("入队 archive_lookup 任务失败: {e}");
        }
        // 预览图：偏好开启时入队下载任务（对齐上游 create_bookmark → load_preview_image）。
        if preview_images_enabled(&state.pool, auth.0.id).await
            && let Err(e) = queue.enqueue(b.id, crate::tasks::queue::KIND_PREVIEW).await
        {
            tracing::warn!("入队 preview 任务失败: {e}");
        }
    }
    let mut v = serde_json::to_value(&b).map_err(AppError::internal)?;
    v["has_preview_image"] = json!(
        b.preview_image_path
            .as_deref()
            .is_some_and(|p| !p.is_empty())
    );
    v["tags"] = json!(tags::tags_for_bookmark(&state.pool, b.id).await?);
    Ok((StatusCode::CREATED, Json(v)).into_response())
}

/// 用户是否开启"启用预览图片"（对齐上游 is_preview_feature_active 的偏好部分）。
async fn preview_images_enabled(pool: &DbPool, user_id: i64) -> bool {
    crate::db::q_scalar::<i64>("SELECT enable_preview_images FROM user_profiles WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| r.get_i64(0) != 0)
        .unwrap_or(false)
}

/// 用户是否开启 Internet Archive 集成（对齐上游 is_web_archive_integration_active）。
async fn web_archive_integration_enabled(pool: &DbPool, user_id: i64) -> bool {
    matches!(
        crate::db::q_scalar::<String>(
            "SELECT web_archive_integration FROM user_profiles WHERE user_id = ?",
        )
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| r.get_string(0)),
        Some(ref v) if v == "enabled"
    )
}

/// 用户是否开启"自动创建快照"（对齐上游 enable_automatic_html_snapshots）。
async fn automatic_snapshots_enabled(pool: &DbPool, user_id: i64) -> bool {
    crate::db::q_scalar::<i64>(
        "SELECT enable_automatic_snapshots FROM user_profiles WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|r| r.get_i64(0) != 0)
    .unwrap_or(false)
}

/// `PATCH /api/v1/bookmarks/:id`
pub async fn update(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<UpdateBookmark>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(url) = &req.url {
        if url.trim().is_empty() {
            return Err(AppError::BadRequest("URL 不能为空".into()));
        }
        if !state.config.disable_url_validation
            && let Err(msg) = crate::bookmarks::normalize::validate_bookmark_url(url)
        {
            return Err(AppError::BadRequest(msg));
        }
    }
    let b = crud::update(&state.pool, auth.0.id, id, &req)
        .await?
        .ok_or(AppError::NotFound)?;
    // 对齐上游 update_bookmark：URL 变更后旧预览图作废，重新触发预览图下载
    //（上游 load_preview_image 在每次 update 都会调用，任务内部覆盖写）。
    if req.url.is_some()
        && !state.config.disable_background_tasks
        && let Some(queue) = &state.task_queue
        && preview_images_enabled(&state.pool, auth.0.id).await
    {
        let _ = crate::db::q("UPDATE bookmarks SET preview_image_path = NULL WHERE id = ?")
            .bind(b.id)
            .execute(&state.pool)
            .await;
        if let Err(e) = queue.enqueue(b.id, crate::tasks::queue::KIND_PREVIEW).await {
            tracing::warn!("入队 preview 任务失败: {e}");
        }
    }
    if req.url.is_some()
        && !state.config.disable_background_tasks
        && let Some(queue) = &state.task_queue
        && web_archive_integration_enabled(&state.pool, auth.0.id).await
        && let Err(e) = queue
            .enqueue(b.id, crate::tasks::queue::KIND_ARCHIVE_LOOKUP)
            .await
    {
        tracing::warn!("入队 archive_lookup 任务失败: {e}");
    }
    let mut v = serde_json::to_value(&b).map_err(AppError::internal)?;
    v["has_preview_image"] = json!(
        b.preview_image_path
            .as_deref()
            .is_some_and(|p| !p.is_empty())
    );
    v["tags"] = json!(tags::tags_for_bookmark(&state.pool, b.id).await?);
    Ok(Json(v))
}

/// `DELETE /api/v1/bookmarks/:id`
pub async fn delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    if crud::delete(&state.pool, auth.0.id, id).await? {
        Ok(Json(json!({"ok": true})).into_response())
    } else {
        Err(AppError::NotFound)
    }
}

/// `POST /api/v1/bookmarks/:id/archive`：归档。
pub async fn archive(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let b = crud::set_archived(&state.pool, auth.0.id, id, true)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::to_value(&b).map_err(AppError::internal)?))
}

/// `POST /api/v1/bookmarks/:id/unarchive`：恢复。
pub async fn unarchive(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let b = crud::set_archived(&state.pool, auth.0.id, id, false)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(serde_json::to_value(&b).map_err(AppError::internal)?))
}

/// `GET /api/v1/bookmarks/tags`：标签列表 + 使用计数。
/// 带搜索参数（q/filter/bundle/tags_any）时，返回当前过滤结果书签使用的标签（对齐 linkding 标签云）。
pub async fn list_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let has_query = params.q.is_some()
        || params.filter.is_some()
        || params.bundle.is_some()
        || params.tags_any.is_some()
        || params.tag.is_some();
    let results = if has_query {
        // 复用列表的搜索条件解析（bundle / URL 覆盖规则一致）。
        let mut bundle_search = crate::bookmarks::bundle::BundleSearch::default();
        if let Some(bid) = params.bundle {
            match crate::bookmarks::bundle::get(&state.pool, auth.0.id, bid).await? {
                Some(b) => {
                    bundle_search =
                        serde_json::from_str(&b.search_json).map_err(AppError::internal)?;
                }
                None => return Err(AppError::NotFound),
            }
        }
        let q = params.q.or(bundle_search.q.clone());
        let filter = params.filter.or(bundle_search.filter.clone());
        // 标签搜索模式与解析失败语义与列表一致（解析失败 → 空标签集）。
        let lax_tag_search =
            crate::db::q_scalar::<String>("SELECT tag_search FROM user_profiles WHERE user_id = ?")
                .bind(auth.0.id)
                .fetch_optional(&state.pool)
                .await?
                .map(|r| r.get_string(0) == "lax")
                .unwrap_or(false);
        let expr = match q.as_deref() {
            Some(q) if !q.trim().is_empty() => match crate::bookmarks::search::parse_query(q) {
                Ok(e) => Some(e),
                Err(_) => {
                    return Ok(Json(json!({ "results": [] })));
                }
            },
            _ => None,
        };
        let lp = ListParams {
            filter,
            tags_and: params
                .tag
                .map(|t| vec![t])
                .unwrap_or_else(|| bundle_search.tags_and.clone()),
            tags_or: if params.tags_any.is_some() {
                params
                    .tags_any
                    .map(|s| {
                        s.split(',')
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty())
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                bundle_search.tags_or.clone()
            },
            tags_not: bundle_search.tags_not.clone(),
            shared: bundle_search.shared,
            unread: None,
            q,
            expr,
            lax_tag_search,
            sort: None,
            page: 1,
            per_page: 1,
            modified_since: params.modified_since.clone(),
            added_since: params.added_since.clone(),
        };
        let mut items =
            crate::bookmarks::crud::list_tags_for_query(&state.pool, auth.0.id, &lp).await?;
        // 排序对齐 list_tags：英文在前，中文拼音序在后。
        let tag_items: Vec<crate::bookmarks::tags::TagWithCount> = items
            .drain(..)
            .map(|(name, cnt)| crate::bookmarks::tags::TagWithCount {
                id: 0,
                name,
                usage_count: cnt,
            })
            .collect();
        let mut tag_items = tag_items;
        crate::bookmarks::tags::sort_tags_english_first(&mut tag_items);
        tag_items
    } else {
        tags::list_tags(&state.pool, auth.0.id).await?
    };
    Ok(Json(json!({ "results": results })))
}

/// `POST /api/v1/bookmarks/tags`：新建标签（已存在则幂等返回）。
pub async fn create_tag(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<serde_json::Value>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), AppError> {
    let name = req
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("name 字段必填".into()))?;
    let mut tx = state.pool.begin().await?;
    let id = tags::get_or_create_tag(&mut tx, auth.0.id, name).await?;
    tx.commit().await?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(json!({"id": id, "name": name.trim()})),
    ))
}

/// `GET /api/v1/bookmarks/tags/{name}`：查询单个标签详情。
pub async fn get_tag(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 大小写不敏感匹配用 LOWER(name) = ?（替代 SQLite 专有 COLLATE NOCASE，三库通用）。
    let id: Option<i64> =
        crate::db::q_scalar::<()>("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
            .bind(auth.0.id)
            .bind(name.trim().to_lowercase())
            .fetch_optional(&state.pool)
            .await?
            .map(|r| r.get_i64(0));
    match id {
        Some(id) => Ok(Json(json!({"id": id, "name": name.trim()}))),
        None => Err(AppError::NotFound),
    }
}

/// `DELETE /api/v1/bookmarks/tags/{name}`：删除标签并解除关联。
pub async fn delete_tag(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let deleted = tags::delete_tag(&state.pool, auth.0.id, &name).await?;
    if deleted {
        Ok(Json(json!({"ok": true})))
    } else {
        Err(AppError::NotFound)
    }
}

/// `PATCH /api/v1/bookmarks/tags/{name}`：重命名标签。
pub async fn rename_tag(
    State(state): State<AppState>,
    auth: AuthUser,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let new_name = req
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("name 字段必填".into()))?;
    tags::rename_tag(&state.pool, auth.0.id, &name, new_name).await?;
    Ok(Json(json!({"ok": true})))
}

/// `POST /api/v1/bookmarks/tags/merge`：合并标签（sources 并入 target，兼容旧 source 单标签）。
pub async fn merge_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    let target = req
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::BadRequest("target 字段必填".into()))?;
    let mut sources: Vec<String> = req
        .get("sources")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if sources.is_empty()
        && let Some(s) = req.get("source").and_then(|v| v.as_str())
    {
        let s = s.trim();
        if !s.is_empty() {
            sources.push(s.to_string());
        }
    }
    if sources.is_empty() {
        return Err(AppError::BadRequest("sources 字段必填".into()));
    }
    tags::merge_tags_multi(&state.pool, auth.0.id, target, &sources).await?;
    Ok(Json(json!({"ok": true})))
}

/// `GET /api/v1/bookmarks/check?url=…&ignore_cache=false`：URL 检查。
/// 对齐 linkding `BookmarkViewSet.check`：返回已存在书签、页面元数据与自动标签预览。
#[derive(Deserialize)]
pub struct CheckQuery {
    pub url: Option<String>,
    #[serde(default)]
    pub ignore_cache: Option<String>,
}

pub async fn check(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<CheckQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let raw_url = params.url.unwrap_or_default();
    if raw_url.trim().is_empty() {
        return Err(AppError::BadRequest("url 参数必填".into()));
    }
    if !state.config.disable_url_validation
        && let Err(msg) = crate::bookmarks::normalize::validate_bookmark_url(&raw_url)
    {
        return Err(AppError::BadRequest(msg));
    }
    // 已存在书签（同用户、同 URL 判重）。

    let normalized = crate::bookmarks::normalize::normalize_url(&raw_url);
    let hash = crate::bookmarks::normalize::url_hash(&normalized);
    let existing_id: Option<i64> =
        crate::db::q_scalar::<()>("SELECT id FROM bookmarks WHERE user_id = ? AND url_hash = ?")
            .bind(auth.0.id)
            .bind(&hash)
            .fetch_optional(&state.pool)
            .await?
            .map(|r| r.get_i64(0));
    let bookmark = match existing_id {
        Some(id) => {
            let b = crud::get(&state.pool, auth.0.id, id)
                .await?
                .ok_or(AppError::NotFound)?;
            let mut v = serde_json::to_value(&b).map_err(AppError::internal)?;
            v["tags"] = json!(tags::tags_for_bookmark(&state.pool, id).await?);
            Some(v)
        }
        None => None,
    };
    // 页面元数据（失败不阻断，返回空元数据）；ignore_cache=true 跳过 LRU。
    let ignore_cache = params
        .ignore_cache
        .as_deref()
        .map(|v| v == "true")
        .unwrap_or(false);
    let fetched = if ignore_cache {
        crate::capture::metadata::fetch_metadata(&raw_url).await
    } else {
        crate::capture::metadata::fetch_metadata_cached(&raw_url).await
    };
    let metadata = match fetched {
        Ok(m) => json!({
            "title": m.title,
            "description": m.description,
            "favicon_url": m.favicon_url,
            "preview_image_url": m.preview_image_url,
        }),
        Err(e) => {
            tracing::warn!("check 抓取元数据失败 url={raw_url}: {e}");
            json!({"title": "", "description": "", "favicon_url": null, "preview_image_url": null})
        }
    };
    // 自动标签预览（对齐 auto_tagging.get_tags）。
    let rules = crate::bookmarks::auto_tag::get_rules(&state.pool, auth.0.id).await?;
    let auto_tags = crate::bookmarks::auto_tag::match_tags(&rules, &raw_url, "");
    Ok(Json(json!({
        "bookmark": bookmark,
        "metadata": metadata,
        "auto_tags": auto_tags,
    })))
}

/// `GET /api/v1/bookmarks/tags/complete?prefix=r`：前缀自动补全。
pub async fn complete_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<CompleteQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let tags = tags::complete_tags(
        &state.pool,
        auth.0.id,
        params.prefix.as_deref().unwrap_or(""),
        params.limit.unwrap_or(10),
    )
    .await?;
    Ok(Json(json!({ "results": tags })))
}

/// `POST /api/v1/bookmarks/bulk/tags/add`：批量加标签。
pub async fn bulk_add_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkTagsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::add_tags(&state.pool, auth.0.id, &req.ids, &req.tags).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/tags/remove`：批量删标签。
pub async fn bulk_remove_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkTagsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r =
        crate::bookmarks::bulk::remove_tags(&state.pool, auth.0.id, &req.ids, &req.tags).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/mark-read`：批量标记已读。
pub async fn bulk_mark_read(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::mark_read(&state.pool, auth.0.id, &req.ids, false).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/mark-unread`：批量标记未读。
pub async fn bulk_mark_unread(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::mark_read(&state.pool, auth.0.id, &req.ids, true).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/delete`：批量删除。
pub async fn bulk_delete(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::delete_many(&state.pool, auth.0.id, &req.ids).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/share`：批量共享。
pub async fn bulk_share(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::set_shared(&state.pool, auth.0.id, &req.ids, true).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/unshare`：批量取消共享。
pub async fn bulk_unshare(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::set_shared(&state.pool, auth.0.id, &req.ids, false).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/refresh`：批量从网站刷新元数据（对齐原版 bulk_refresh）。
pub async fn bulk_refresh(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if state.config.disable_background_tasks {
        return Err(AppError::BadRequest(
            "后台任务已禁用（disable_background_tasks）".into(),
        ));
    }
    let queue = state
        .task_queue
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("任务队列未启用".into()))?;
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut results = Vec::new();
    for &id in &req.ids {
        let owns =
            crate::db::q_scalar::<i64>("SELECT id FROM bookmarks WHERE id = ? AND user_id = ?")
                .bind(id)
                .bind(auth.0.id)
                .fetch_optional(&state.pool)
                .await?;
        if owns.is_none() {
            failed += 1;
            results.push(serde_json::json!({"id": id, "ok": false, "error": "not found"}));
            continue;
        }
        match queue.enqueue(id, "metadata").await {
            Ok(_) => {
                succeeded += 1;
                results.push(serde_json::json!({"id": id, "ok": true}));
            }
            Err(e) => {
                failed += 1;
                results.push(serde_json::json!({"id": id, "ok": false, "error": e.message()}));
            }
        }
        // 对齐上游 refresh_bookmarks_metadata：刷新元数据时同时补抓预览图。
        if preview_images_enabled(&state.pool, auth.0.id).await
            && let Err(e) = queue.enqueue(id, crate::tasks::queue::KIND_PREVIEW).await
        {
            tracing::warn!("入队 preview 任务失败 id={id}: {e}");
        }
    }
    Ok(Json(
        serde_json::json!({"results": results, "succeeded": succeeded, "failed": failed}),
    ))
}

/// `PUT /api/v1/bookmarks/:id/tags`：整体替换书签标签。
pub async fn set_tags(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<SetTagsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    tags::set_bookmark_tags(&state.pool, auth.0.id, id, &req.tags).await?;
    let list = tags::tags_for_bookmark(&state.pool, id).await?;
    Ok(Json(json!({ "tags": list })))
}

/// 列表查询字符串。
#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub filter: Option<String>,
    pub tag: Option<String>,
    /// 逗号分隔的 OR 标签（`tags_any=rust,web`）。
    pub tags_any: Option<String>,
    pub q: Option<String>,
    pub sort: Option<String>,
    /// 共享过滤（对齐原版 shared=yes/no/off）：yes=仅共享，no=仅未共享。
    pub shared: Option<String>,
    /// 未读过滤（对齐原版 unread=yes/no/off）：yes=仅未读，no=仅已读。
    pub unread: Option<String>,
    /// Bundle（保存的过滤器）id：应用该 bundle 保存的搜索条件。
    pub bundle: Option<i64>,
    pub page: Option<i64>,
    /// 仅返回 updated_at 晚于该 RFC3339 时间的书签（对齐上游 modified_since）。
    pub modified_since: Option<String>,
    /// 仅返回 created_at 晚于该 RFC3339 时间的书签（对齐上游 added_since）。
    pub added_since: Option<String>,
    pub per_page: Option<i64>,
}

#[derive(Deserialize)]
pub struct CompleteQuery {
    pub prefix: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct SetTagsRequest {
    #[serde(default)]
    pub tags: Vec<String>,
}

/// `POST /api/v1/bookmarks/bulk/archive`：批量归档。
pub async fn bulk_archive(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::set_archived(&state.pool, auth.0.id, &req.ids, true).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/unarchive`：批量取消归档。
pub async fn bulk_unarchive(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let r = crate::bookmarks::bulk::set_archived(&state.pool, auth.0.id, &req.ids, false).await?;
    Ok(Json(crate::bookmarks::bulk::to_json(&r)))
}

/// `POST /api/v1/bookmarks/bulk/snapshot`：批量创建 HTML 快照（对齐上游 bulk_snapshot）。
/// 逐个入队 snapshot 任务（服务端抓取），返回逐条结果。
pub async fn bulk_snapshot(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<crate::bookmarks::bulk::BulkIdsRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !state.config.enable_snapshots {
        return Err(AppError::BadRequest(
            "快照功能未启用（enable_snapshots）".into(),
        ));
    }
    if state.config.disable_background_tasks {
        return Err(AppError::BadRequest(
            "后台任务已禁用（disable_background_tasks）".into(),
        ));
    }
    let queue = state
        .task_queue
        .as_ref()
        .ok_or_else(|| AppError::BadRequest("任务队列未启用".into()))?;

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut results = Vec::new();
    for &id in &req.ids {
        let owns =
            crate::db::q_scalar::<i64>("SELECT id FROM bookmarks WHERE id = ? AND user_id = ?")
                .bind(id)
                .bind(auth.0.id)
                .fetch_optional(&state.pool)
                .await?;
        if owns.is_none() {
            failed += 1;
            results.push(serde_json::json!({"id": id, "ok": false, "error": "not found"}));
            continue;
        }
        match queue.enqueue(id, crate::tasks::queue::KIND_SNAPSHOT).await {
            Ok(_) => {
                succeeded += 1;
                results.push(serde_json::json!({"id": id, "ok": true}));
            }
            Err(e) => {
                failed += 1;
                results.push(serde_json::json!({"id": id, "ok": false, "error": e.message()}));
            }
        }
    }
    Ok(Json(
        serde_json::json!({"results": results, "succeeded": succeeded, "failed": failed}),
    ))
}
