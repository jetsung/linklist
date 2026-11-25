use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::db::{DbDialect, DbQuery};
use crate::error::AppError;
use crate::web::auth_api::AppState;
use crate::web::profile_api::effective_profile;

#[derive(Deserialize, Default)]
pub struct SharedQuery {
    pub tag: Option<String>,
    pub q: Option<String>,
    /// 按共享者用户名筛选（对齐原版 ?user=）。
    pub user: Option<String>,
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    /// 排序（对齐原版 BookmarkSearch.sort 六值）。
    pub sort: Option<String>,
}

/// 解析 q：优先搜索表达式（#标签/tag:/!特殊关键字/AND/OR/NOT），否则退化为关键词 LIKE。
/// 对齐 bookmarks_api 的搜索解析；多标签（#A1 #A2）为 AND 组合。
fn parse_shared_query(
    q: &Option<String>,
) -> (Option<crate::bookmarks::search::Expr>, Option<String>) {
    match q.as_deref() {
        Some(q) if !q.trim().is_empty() => match crate::bookmarks::search::parse_query(q) {
            Ok(expr) => (Some(expr), None),
            Err(_) => (None, Some(q.to_string())),
        },
        _ => (None, None),
    }
}

/// 把表达式编译产出的绑定值按序附加到跨后端查询。
fn bind_strs(query: DbQuery, binds: &[String]) -> DbQuery {
    let mut query = query;
    for b in binds {
        query = query.bind(b.as_str());
    }
    query
}

/// 关键词 icontains 条件（四列，绑定小写 LIKE 模式；三库通用的 LOWER(col) LIKE ?）。
const KEYWORD_COND: &str = "(LOWER(b.title) LIKE ? OR LOWER(b.url) LIKE ? \
  OR LOWER(b.description) LIKE ? OR LOWER(b.notes) LIKE ?)";

/// 共享列表排序 SQL（登录态 `/api/v1/shared` 与公开 `/api/v1/shared/public` 共用）。
/// 标题排序大小写不敏感：LOWER(b.title)（替代 SQLite 专有 COLLATE NOCASE，三库通用）。
fn shared_order_sql(sort: Option<&str>) -> &'static str {
    match sort {
        Some("added_asc") => "ORDER BY b.created_at ASC",
        Some("added_desc") | Some("created") => "ORDER BY b.created_at DESC",
        Some("modified_asc") => "ORDER BY b.updated_at ASC",
        Some("modified_desc") | Some("updated") => "ORDER BY b.updated_at DESC",
        Some("title_asc") | Some("title") => "ORDER BY LOWER(b.title) ASC",
        Some("title_desc") => "ORDER BY LOWER(b.title) DESC",
        _ => "ORDER BY b.created_at DESC",
    }
}

fn q_like(q: &Option<String>) -> Option<String> {
    q.as_ref().map(|q| format!("%{}%", q.to_lowercase()))
}

fn row_json(r: &crate::db::DbRow, viewer_id: Option<i64>) -> serde_json::Value {
    let tags: Vec<String> = r
        .get_named_string("tags_agg")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let owner_id: i64 = r.get_named_i64("user_id");
    let snapshot_path: Option<String> = r.get_named_opt_string("snapshot_path");
    let preview_image_path: Option<String> = r.get_named_opt_string("preview_image_path");
    serde_json::json!({
        "id": r.get_named_i64("id"),
        "user_id": owner_id,
        "username": r.get_named_string("username"),
        "url": r.get_named_string("url"),
        "title": r.get_named_string("title"),
        "description": r.get_named_string("description"),
        "notes": r.get_named_string("notes"),
        "is_unread": r.get_named_bool("is_unread"),
        "is_shared": true,
        "is_archived": false,
        // 操作判定字段：是否当前登录用户所有（公开匿名视图恒为 false）。
        "is_editable": viewer_id.map(|v| v == owner_id).unwrap_or(false),
        // 快照信息：本地 HTML 快照 / Internet Archive 回填地址（可为 null）。
        "has_snapshot": snapshot_path.is_some(),
        "archive_url": r.get_named_opt_string("archive_url"),
        // 预览图：有本地缓存时前端渲染 /previews/{id}（对齐上游 show_preview_images）。
        "has_preview_image": preview_image_path.as_deref().is_some_and(|p| !p.is_empty()),
        "tags": tags,
        "created_at": r.get_named_string("created_at"),
    })
}

/// tags_agg 列表达式（按方言选择聚合拼接写法）。
fn tags_agg_sql(dialect: DbDialect) -> String {
    let concat = crate::db::sql_concat_agg(dialect, "t.name", ",");
    format!(
        "(SELECT {concat} FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id WHERE bt.bookmark_id = b.id) AS tags_agg"
    )
}

/// `GET /api/v1/shared`：共享书签列表（支持标签/关键词/用户筛选）。
/// 登录：所有开启共享用户的共享书签（含自己的，只读）；匿名：仅公开共享（enable_public_sharing）视图。
pub async fn list_shared(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    Query(params): Query<SharedQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let dialect = state.pool.dialect();
    let viewer = crate::web::auth_api::current_user(&state.pool, &session).await?;
    let page = params.page.unwrap_or(1).max(1);
    // 未显式传 per_page 时用"生效资料"的每页条数偏好
    // （登录=本人资料；匿名=访客资料 guest_profile_user；对齐上游 Paginator(items_per_page)）。
    let per_page = match params.per_page {
        Some(p) => p.clamp(1, 200),
        None => effective_profile(&state.pool, viewer.as_ref().map(|u| u.id))
            .await
            .items_per_page
            .clamp(1, 200),
    };
    // q 解析：搜索表达式优先（#标签 AND/OR/NOT），失败或非表达式时按关键词 LIKE。
    let (expr, q_keyword) = parse_shared_query(&params.q);
    let mut cond_binds: Vec<String> = Vec::new();
    let expr_sql = expr
        .as_ref()
        .map(|e| crate::bookmarks::search::compile(e, &mut cond_binds));
    let q_like = q_like(&q_keyword.clone());

    // 条件：共享 + 未归档 + 用户筛选 + 标签（tag= 参数）+ 表达式/关键词。
    // 布尔比较统一 TRUE/FALSE 字面量（PG BOOLEAN 不能与整数比较）。
    let mut conds: Vec<String> = vec![
        "b.is_shared = TRUE".into(),
        "b.is_archived = FALSE".into(),
        "(? IS NULL OR u.username = ?)".into(),
        "up.enable_sharing = TRUE".into(),
        "COALESCE(u.is_active, TRUE) = TRUE".into(),
    ];
    // 匿名访问 = 公开共享视图（仅含开启 enable_public_sharing 的用户）。
    if viewer.is_none() {
        conds.push("up.enable_public_sharing = TRUE".into());
    }
    if let Some(tag) = &params.tag {
        conds.push(
            "EXISTS (SELECT 1 FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id \
             WHERE bt.bookmark_id = b.id AND LOWER(t.name) = ?)"
                .into(),
        );
        cond_binds.push(tag.to_lowercase());
    }
    if let Some(es) = &expr_sql {
        conds.push(es.clone());
    } else if let Some(ql) = &q_like {
        conds.push(KEYWORD_COND.into());
        for _ in 0..4 {
            cond_binds.push(ql.clone());
        }
    }
    let where_sql = conds.join(" AND ");
    let base = format!(
        "FROM bookmarks b JOIN users u ON u.id = b.user_id JOIN user_profiles up ON up.user_id = b.user_id WHERE {where_sql}"
    );
    let total_sql = format!("SELECT COUNT(*) {base}");
    let total_q = crate::db::q(&total_sql)
        .bind(params.user.clone())
        .bind(params.user.clone());
    let total_q = bind_strs(total_q, &cond_binds);
    let total: i64 = total_q.fetch_one(&state.pool).await.map(|r| r.get_i64(0))?;

    // 标题排序大小写不敏感：LOWER(b.title)（替代 SQLite 专有 COLLATE NOCASE，三库通用）。
    let order_sql = shared_order_sql(params.sort.as_deref());
    let list_sql = format!(
        "SELECT b.*, u.username, {} \
         {base} {order_sql} LIMIT ? OFFSET ?",
        tags_agg_sql(dialect)
    );
    let list_q = crate::db::q(&list_sql)
        .bind(params.user.clone())
        .bind(params.user.clone());
    let list_q = bind_strs(list_q, &cond_binds);
    let rows = list_q
        .bind(per_page)
        .bind((page - 1) * per_page)
        .fetch_all(&state.pool)
        .await?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| row_json(r, viewer.as_ref().map(|u| u.id)))
        .collect();
    Ok(Json(serde_json::json!({
        "results": items,
        "total": total,
        "page": page,
        "per_page": per_page,
    })))
}

/// `GET /api/v1/shared/tags`：共享书签的标签统计（对齐 SharedTagCloudContext）。
pub async fn list_shared_tags(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    Query(params): Query<SharedQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let viewer = crate::web::auth_api::current_user(&state.pool, &session).await?;
    let (expr, q_keyword) = parse_shared_query(&params.q);
    let mut cond_binds: Vec<String> = Vec::new();
    let expr_sql = expr
        .as_ref()
        .map(|e| crate::bookmarks::search::compile(e, &mut cond_binds));
    let q_like = q_like(&q_keyword.clone());
    let mut conds: Vec<String> = vec![
        "b.is_shared = TRUE".into(),
        "(? IS NULL OR u.username = ?)".into(),
        "up.enable_sharing = TRUE".into(),
        "COALESCE(u.is_active, TRUE) = TRUE".into(),
    ];
    if viewer.is_none() {
        conds.push("up.enable_public_sharing = TRUE".into());
    }
    if let Some(es) = &expr_sql {
        conds.push(es.clone());
    } else if let Some(ql) = &q_like {
        conds.push(KEYWORD_COND.into());
        for _ in 0..4 {
            cond_binds.push(ql.clone());
        }
    }
    let where_sql = conds.join(" AND ");
    let join_up = "JOIN user_profiles up ON up.user_id = b.user_id";
    let sql = format!(
        "SELECT t.name, COUNT(DISTINCT b.id) AS cnt
         FROM bookmarks b
         JOIN users u ON u.id = b.user_id
         {join_up}
         JOIN bookmark_tags bt ON bt.bookmark_id = b.id
         JOIN tags t ON t.id = bt.tag_id
         WHERE {where_sql}
         GROUP BY t.name ORDER BY cnt DESC, t.name"
    );
    let q = crate::db::q(&sql)
        .bind(params.user.clone())
        .bind(params.user.clone());
    let q = bind_strs(q, &cond_binds);
    let rows = q.fetch_all(&state.pool).await?;
    // 标签云排序对齐全站规范：英文 A–Z 在前，中文按拼音 A–Z 在后（不按使用次数）。
    let mut tag_items: Vec<crate::bookmarks::tags::TagWithCount> = rows
        .iter()
        .map(|r| crate::bookmarks::tags::TagWithCount {
            id: 0,
            name: r.get_named_string("name"),
            usage_count: r.get_named_i64("cnt"),
        })
        .collect();
    crate::bookmarks::tags::sort_tags_english_first(&mut tag_items);
    let results: Vec<serde_json::Value> = tag_items
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "count": t.usage_count,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "results": results })))
}

/// `GET /api/v1/shared/users`：有共享书签的用户列表（对齐 UserListContext）。
pub async fn list_shared_users(
    State(state): State<AppState>,
    session: tower_sessions::Session,
) -> Result<Json<serde_json::Value>, AppError> {
    let viewer = crate::web::auth_api::current_user(&state.pool, &session).await?;
    let join_up = "JOIN user_profiles up ON up.user_id = b.user_id";
    let anon_cond = if viewer.is_none() {
        "AND up.enable_public_sharing = TRUE"
    } else {
        ""
    };
    let sql = format!(
        "SELECT u.id, u.username, COUNT(b.id) AS cnt
         FROM bookmarks b JOIN users u ON u.id = b.user_id
         {join_up}
         WHERE b.is_shared = TRUE AND b.is_archived = FALSE AND up.enable_sharing = TRUE AND COALESCE(u.is_active, TRUE) = TRUE {anon_cond}
         GROUP BY u.id, u.username HAVING COUNT(b.id) > 0 ORDER BY LOWER(u.username)"
    );
    let rows = crate::db::q(&sql).fetch_all(&state.pool).await?;
    let results: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get_named_i64("id"),
                "username": r.get_named_string("username"),
                "count": r.get_named_i64("cnt"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "results": results })))
}

/// `GET /api/v1/shared/public`：公开共享视图（对齐 linkding `/bookmarks/shared` 匿名可见）。
/// 双层控制：书签 is_shared=1 且所有者 enable_public_sharing=1。
/// 匿名可访问；结果仅含已开启公开共享的用户的书签。
pub async fn list_public_shared(
    State(state): State<AppState>,
    Query(params): Query<SharedQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let dialect = state.pool.dialect();
    let page = params.page.unwrap_or(1).max(1);
    // 未显式传 per_page 时用"生效资料"的每页条数偏好
    // （该接口匿名可访问 → 访客资料 guest_profile_user；对齐上游 Paginator(items_per_page)）。
    let per_page = match params.per_page {
        Some(p) => p.clamp(1, 200),
        None => effective_profile(&state.pool, None)
            .await
            .items_per_page
            .clamp(1, 200),
    };
    // q 解析与登录态接口一致：表达式优先（#多标签 AND），否则关键词。
    let (expr, q_keyword) = parse_shared_query(&params.q);
    let mut cond_binds: Vec<String> = Vec::new();
    let expr_sql = expr
        .as_ref()
        .map(|e| crate::bookmarks::search::compile(e, &mut cond_binds));
    let q_like = q_like(&q_keyword.clone());

    let mut conds: Vec<String> = vec![
        "b.is_shared = TRUE".into(),
        "b.is_archived = FALSE".into(),
        "up.enable_public_sharing = TRUE".into(),
        "COALESCE(u.is_active, TRUE) = TRUE".into(),
    ];
    if let Some(tag) = &params.tag {
        conds.push(
            "EXISTS (SELECT 1 FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id \
             WHERE bt.bookmark_id = b.id AND LOWER(t.name) = ?)"
                .into(),
        );
        cond_binds.push(tag.to_lowercase());
    }
    if let Some(es) = &expr_sql {
        conds.push(es.clone());
    } else if let Some(ql) = &q_like {
        conds.push(KEYWORD_COND.into());
        for _ in 0..4 {
            cond_binds.push(ql.clone());
        }
    }
    let base = format!(
        "FROM bookmarks b
     JOIN user_profiles up ON up.user_id = b.user_id
     JOIN users u ON u.id = b.user_id
     WHERE {}",
        conds.join(" AND ")
    );

    let total_sql = format!("SELECT COUNT(*) {base}");
    let total: i64 = bind_strs(crate::db::q(&total_sql), &cond_binds)
        .fetch_one(&state.pool)
        .await
        .map(|r| r.get_i64(0))?;

    // 与登录态列表一致：附带 username 与 tags_agg（row_json 依赖这两列）；
    // 排序同样响应 sort 参数（与 /api/v1/shared 共用同一套排序语义）。
    let order_sql = shared_order_sql(params.sort.as_deref());
    let list_sql = format!(
        "SELECT b.*, u.username, {} {base} {order_sql} LIMIT ? OFFSET ?",
        tags_agg_sql(dialect)
    );
    let rows = bind_strs(crate::db::q(&list_sql), &cond_binds)
        .bind(per_page)
        .bind((page - 1) * per_page)
        .fetch_all(&state.pool)
        .await?;

    let items: Vec<serde_json::Value> = rows.iter().map(|r| row_json(r, None)).collect();
    Ok(Json(serde_json::json!({
        "results": items,
        "total": total,
        "page": page,
        "per_page": per_page,
    })))
}

/// `GET /api/v1/shared/public/tags`：公开共享书签的标签统计（对齐 SharedTagCloudContext 匿名视角）。
pub async fn list_public_shared_tags(
    State(state): State<AppState>,
    Query(params): Query<SharedQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (expr, q_keyword) = parse_shared_query(&params.q);
    let mut cond_binds: Vec<String> = Vec::new();
    let expr_sql = expr
        .as_ref()
        .map(|e| crate::bookmarks::search::compile(e, &mut cond_binds));
    let q_like = q_like(&q_keyword.clone());
    let mut conds: Vec<String> = vec![
        "b.is_shared = TRUE".into(),
        "b.is_archived = FALSE".into(),
        "up.enable_public_sharing = TRUE".into(),
        "COALESCE(u.is_active, TRUE) = TRUE".into(),
    ];
    if let Some(es) = &expr_sql {
        conds.push(es.clone());
    } else if let Some(ql) = &q_like {
        conds.push(KEYWORD_COND.into());
        for _ in 0..4 {
            cond_binds.push(ql.clone());
        }
    }
    let where_sql = conds.join(" AND ");
    let sql = format!(
        "SELECT t.name, COUNT(DISTINCT b.id) AS cnt
         FROM bookmarks b
         JOIN users u ON u.id = b.user_id
         JOIN user_profiles up ON up.user_id = b.user_id
         JOIN bookmark_tags bt ON bt.bookmark_id = b.id
         JOIN tags t ON t.id = bt.tag_id
         WHERE {where_sql}
         GROUP BY t.name ORDER BY cnt DESC, t.name"
    );
    let q = bind_strs(crate::db::q(&sql), &cond_binds);
    let rows = q.fetch_all(&state.pool).await?;
    // 同 list_shared_tags：英文 A–Z 在前，中文拼音 A–Z 在后。
    let mut tag_items: Vec<crate::bookmarks::tags::TagWithCount> = rows
        .iter()
        .map(|r| crate::bookmarks::tags::TagWithCount {
            id: 0,
            name: r.get_named_string("name"),
            usage_count: r.get_named_i64("cnt"),
        })
        .collect();
    crate::bookmarks::tags::sort_tags_english_first(&mut tag_items);
    let results: Vec<serde_json::Value> = tag_items
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "count": t.usage_count,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "results": results })))
}

/// `GET /api/v1/shared/viewer-profile`：共享视图的"生效资料"（匿名可访问）。
/// 登录用户返回本人资料与搜索偏好；匿名返回访客资料（guest_profile_user）或标准资料，
/// 供前端在静态页面上按访客偏好渲染（对齐上游 LinkdingMiddleware 注入的 request.user_profile）。
pub async fn viewer_profile(
    State(state): State<AppState>,
    session: tower_sessions::Session,
) -> Result<Json<serde_json::Value>, AppError> {
    let viewer = crate::web::auth_api::current_user(&state.pool, &session).await?;
    let viewer_id = viewer.as_ref().map(|u| u.id);
    let p = effective_profile(&state.pool, viewer_id).await;
    // 搜索偏好：登录取本人的；匿名取访客资料指向用户保存的偏好（对齐上游
    // BookmarkSearch.from_request(request, GET, user_profile.search_preferences)）。
    let prefs_user = viewer_id.or(p.user_id);
    let search_prefs = match prefs_user {
        Some(uid) => {
            let key = format!("search_prefs:{uid}");
            let raw: Option<String> = crate::db::q("SELECT value FROM settings WHERE key = ?")
                .bind(key.as_str())
                .fetch_optional(&state.pool)
                .await?
                .map(|r| r.get_named_opt_string("value").unwrap_or_default())
                .filter(|s| !s.is_empty());
            raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        }
        None => None,
    }
    .unwrap_or_else(|| serde_json::json!({"sort": "added_desc", "shared": "off", "unread": "off"}));
    Ok(Json(serde_json::json!({
        "anonymous": viewer.is_none(),
        "theme": p.theme,
        "items_per_page": p.items_per_page,
        "custom_css": p.custom_css,
        "enable_favicons": p.enable_favicons,
        "enable_preview_images": p.enable_preview_images,
        "bookmark_date_display": p.bookmark_date_display,
        "bookmark_description_display": p.bookmark_description_display,
        "display_url": p.display_url,
        "search_preferences": search_prefs,
    })))
}
