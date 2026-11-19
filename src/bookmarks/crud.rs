use crate::db::DbPool;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::bookmarks::normalize::{normalize_url, url_hash};
use crate::error::AppError;

/// 书签完整记录。
#[derive(Debug, Clone, Serialize)]
pub struct Bookmark {
    pub id: i64,
    pub user_id: i64,
    pub url: String,
    pub title: String,
    pub description: String,
    pub notes: String,
    pub is_unread: bool,
    pub is_archived: bool,
    pub is_shared: bool,
    #[serde(skip_serializing)]
    pub url_hash: String,
    /// 预留：站点图标本地缓存路径（当前由抓取任务写入，界面暂未消费）。
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub favicon_path: Option<String>,
    /// 预留：预览图本地缓存路径。
    #[serde(skip_serializing)]
    #[allow(dead_code)]
    pub preview_image_path: Option<String>,
    #[serde(skip_serializing)]
    /// 快照文件相对路径（抓取任务写入；界面通过 /snapshot 端点读取）。
    #[allow(dead_code)]
    pub snapshot_path: Option<String>,
    pub archive_url: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 创建书签请求。
#[derive(Debug, Deserialize)]
pub struct NewBookmark {
    pub url: String,
    /// 允许省略或显式 null（前端表单会传 null），空串/None 时回退 URL。
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub is_unread: bool,
    #[serde(default)]
    pub is_shared: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 更新书签请求（字段可选，None 表示不改）。
#[derive(Debug, Deserialize, Default)]
pub struct UpdateBookmark {
    pub url: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub is_unread: Option<bool>,
    pub is_archived: Option<bool>,
    pub is_shared: Option<bool>,
}

/// 列表查询参数。
#[derive(Debug, Default, Clone)]
pub struct ListParams {
    /// 状态筛选：unread / archived / all（默认未读优先的活跃书签）。
    pub filter: Option<String>,
    /// 标签筛选（AND 语义）。
    pub tags_and: Vec<String>,
    /// 标签筛选（OR 语义）。
    pub tags_or: Vec<String>,
    /// 标签排除（任一命中即排除）。
    pub tags_not: Vec<String>,
    /// 共享状态筛选：Some(true)=仅共享，Some(false)=仅未共享。
    pub shared: Option<bool>,
    /// 未读状态筛选：Some(true)=仅未读，Some(false)=仅已读。
    pub unread: Option<bool>,
    pub q: Option<String>,
    /// 解析后的搜索表达式（优先于 q 的 LIKE 兜底）。
    pub expr: Option<crate::bookmarks::search::Expr>,
    /// 标签搜索模式（对齐上游 tag_search）：lax 时普通词项兼查标签名。
    pub lax_tag_search: bool,
    /// 排序（对齐原版 BookmarkSearch）：added_asc/added_desc/modified_asc/modified_desc/title_asc/title_desc；兼容旧值 created/updated/title。
    pub sort: Option<String>,
    pub page: i64,
    pub per_page: i64,
    /// 仅返回 updated_at 晚于该 RFC3339 时间的书签（解析失败时忽略）。
    pub modified_since: Option<String>,
    /// 仅返回 created_at 晚于该 RFC3339 时间的书签（解析失败时忽略）。
    pub added_since: Option<String>,
}

impl ListParams {
    pub fn offset(&self) -> i64 {
        let page = if self.page < 1 { 1 } else { self.page };
        let per = self.effective_per_page();
        (page - 1) * per
    }

    pub fn effective_per_page(&self) -> i64 {
        if self.per_page < 1 {
            30
        } else if self.per_page > 200 {
            200
        } else {
            self.per_page
        }
    }
}

/// 创建书签。同一用户下规范化 URL 重复 → Conflict。
pub async fn create(pool: &DbPool, user_id: i64, req: &NewBookmark) -> Result<Bookmark, AppError> {
    let normalized = normalize_url(&req.url);
    let hash = url_hash(&normalized);
    let now = Utc::now().to_rfc3339();
    let title = match req.title.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => normalized.clone(),
    };

    // 判重。
    let existing: Option<i64> =
        crate::db::q_scalar::<()>("SELECT id FROM bookmarks WHERE user_id = ? AND url_hash = ?")
            .bind(user_id)
            .bind(&hash)
            .fetch_optional(pool)
            .await?
            .map(|r| r.get_i64(0));
    if existing.is_some() {
        return Err(AppError::Conflict("相同 URL 的书签已存在".into()));
    }

    // 事务外先读自动打标规则（避免事务中再从池借连接）。
    let rules = crate::bookmarks::auto_tag::get_rules(pool, user_id).await?;

    let mut tx = pool.begin().await?;
    let id = match pool {
        DbPool::Postgres(_) => {
            let row = crate::db::q(
                "INSERT INTO bookmarks (user_id, url, url_hash, title, description, notes, is_unread, is_shared, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
            )
            .bind(user_id)
            .bind(&normalized)
            .bind(&hash)
            .bind(&title)
            .bind(req.description.as_deref().unwrap_or("").trim())
            .bind(req.notes.as_deref().unwrap_or(""))
            .bind(req.is_unread)
            .bind(req.is_shared)
            .bind(&now)
            .bind(&now)
            .fetch_one_tx(&mut tx)
            .await?;
            row.get_i64(0)
        }
        // SQLite 与 MySQL 同形态：INSERT 后用响应自带的 last_insert_id 回读主键
        // （MySQL 无 RETURNING；LAST_INSERT_ID 连接级原子，事务内可靠）。
        DbPool::Sqlite(_) | DbPool::MySql(_) => {
            let result = crate::db::q(
                "INSERT INTO bookmarks (user_id, url, url_hash, title, description, notes, is_unread, is_shared, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(&normalized)
            .bind(&hash)
            .bind(&title)
            .bind(req.description.as_deref().unwrap_or("").trim())
            .bind(req.notes.as_deref().unwrap_or(""))
            .bind(req.is_unread as i64)
            .bind(req.is_shared as i64)
            .bind(&now)
            .bind(&now)
            .execute_tx(&mut tx)
            .await?;
            result.last_insert_rowid()
        }
    };

    // 打标签（含自动打标规则命中附加）。
    let mut all_tags = req.tags.clone();
    crate::bookmarks::auto_tag::apply_rules(&rules, &normalized, &title, &mut all_tags);
    for tag_name in &all_tags {
        let tag_id = crate::bookmarks::tags::get_or_create_tag(&mut tx, user_id, tag_name).await?;
        crate::db::q(crate::db::sql_insert_ignore(
            tx.dialect(),
            "bookmark_tags (bookmark_id, tag_id)",
            "?, ?",
        ))
        .bind(id)
        .bind(tag_id)
        .execute_tx(&mut tx)
        .await?;
    }

    tx.commit().await?;
    get(pool, user_id, id)
        .await?
        .ok_or_else(|| AppError::internal("创建后读取书签失败"))
}

/// 按 id 获取书签（限本人）。
pub async fn get(pool: &DbPool, user_id: i64, id: i64) -> Result<Option<Bookmark>, AppError> {
    let row = crate::db::q(
        "SELECT id, user_id, url, url_hash, title, description, notes, is_unread, is_archived, is_shared,
                favicon_path, preview_image_path, snapshot_path, archive_url, created_at, updated_at
         FROM bookmarks WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .map(row_to_bookmark);
    Ok(row)
}

/// 更新书签（限本人）。
pub async fn update(
    pool: &DbPool,
    user_id: i64,
    id: i64,
    req: &UpdateBookmark,
) -> Result<Option<Bookmark>, AppError> {
    let existing = get(pool, user_id, id).await?.ok_or(AppError::NotFound)?;
    let url = req
        .url
        .as_ref()
        .map(|u| normalize_url(u))
        .unwrap_or(existing.url);
    let hash = url_hash(&url);
    let title = req.title.clone().unwrap_or(existing.title);
    let description = req.description.clone().unwrap_or(existing.description);
    let notes = req.notes.clone().unwrap_or(existing.notes);
    let is_unread = req.is_unread.unwrap_or(existing.is_unread);
    let is_archived = req.is_archived.unwrap_or(existing.is_archived);
    let is_shared = req.is_shared.unwrap_or(existing.is_shared);
    let now = Utc::now().to_rfc3339();

    // URL 变更时判重（排除自身）。
    if hash != existing.url_hash {
        let dup: Option<i64> = crate::db::q_scalar::<()>(
            "SELECT id FROM bookmarks WHERE user_id = ? AND url_hash = ? AND id != ?",
        )
        .bind(user_id)
        .bind(&hash)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .map(|r| r.get_i64(0));
        if dup.is_some() {
            return Err(AppError::Conflict("相同 URL 的书签已存在".into()));
        }
    }
    let existing_tags = crate::bookmarks::tags::tags_for_bookmark(pool, id).await?;
    crate::db::q(
        "UPDATE bookmarks SET url = ?, url_hash = ?, title = ?, description = ?, notes = ?, is_unread = ?, is_archived = ?, is_shared = ?, updated_at = ? WHERE id = ? AND user_id = ?",
    )
    .bind(&url)
    .bind(&hash)
    .bind(&title)
    .bind(&description)
    .bind(&notes)
    .bind(is_unread as i64)
    .bind(is_archived as i64)
    .bind(is_shared as i64)
    .bind(&now)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;

    // 自动打标：更新时也按规则追加命中标签（不删除已有标签，对齐上游 _update_bookmark_tags）。
    let rules = crate::bookmarks::auto_tag::get_rules(pool, user_id).await?;
    if !rules.is_empty() {
        let mut auto_tags = crate::bookmarks::auto_tag::match_tags(&rules, &url, &title);
        auto_tags.retain(|t| !existing_tags.iter().any(|x| x.eq_ignore_ascii_case(t)));
        if !auto_tags.is_empty() {
            let mut merged = existing_tags.clone();
            merged.extend(auto_tags);
            crate::bookmarks::tags::set_bookmark_tags(pool, user_id, id, &merged).await?;
        }
    }

    get(pool, user_id, id).await
}

/// 删除书签（限本人）。
pub async fn delete(pool: &DbPool, user_id: i64, id: i64) -> Result<bool, AppError> {
    // 先取快照/图标路径（删除后清理文件）。
    let paths: Option<(Option<String>, Option<String>)> = crate::db::q_as::<()>(
        "SELECT snapshot_path, favicon_path FROM bookmarks WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?
    .map(|r| {
        (
            r.get_named_opt_string("snapshot_path"),
            r.get_named_opt_string("favicon_path"),
        )
    });
    let result = crate::db::q("DELETE FROM bookmarks WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    let deleted = result.rows_affected() > 0;
    if deleted {
        // 清理快照与图标文件（尽力而为，失败不影响删除结果）。
        if let Some((snapshot, favicon)) = paths {
            for p in [snapshot, favicon].into_iter().flatten() {
                let path = std::path::Path::new(&p);
                if path.is_absolute() {
                    let _ = tokio::fs::remove_file(path).await;
                }
            }
        }
    }
    Ok(deleted)
}

/// 归档/恢复。
pub async fn set_archived(
    pool: &DbPool,
    user_id: i64,
    id: i64,
    archived: bool,
) -> Result<Option<Bookmark>, AppError> {
    let now = Utc::now().to_rfc3339();
    crate::db::q(
        "UPDATE bookmarks SET is_archived = ?, updated_at = ? WHERE id = ? AND user_id = ?",
    )
    .bind(archived as i64)
    .bind(&now)
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    get(pool, user_id, id).await
}

/// 列表过滤绑定值（标签字符串 / 计数整数 / FTS 关键词字符串）。
pub(crate) enum ListBind {
    S(String),
    I(i64),
}

/// 构建 list / 标签统计共用的 WHERE 子句与按序绑定值。
pub(crate) fn build_list_filters(params: &ListParams) -> (String, Vec<ListBind>) {
    let mut where_clauses: Vec<String> = vec!["b.user_id = ?".into()];
    let mut binds: Vec<ListBind> = Vec::new();
    let filter = params.filter.as_deref().unwrap_or("active");
    match filter {
        "unread" => where_clauses.push("b.is_unread = TRUE AND b.is_archived = FALSE".into()),
        "archived" => where_clauses.push("b.is_archived = 1".into()),
        "all" => {}
        _ => where_clauses.push("b.is_archived = FALSE".into()),
    }
    if !params.tags_and.is_empty() {
        let placeholders = params
            .tags_and
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        where_clauses.push(format!(
            "b.id IN (SELECT bt.bookmark_id FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id WHERE LOWER(t.name) IN ({placeholders}) GROUP BY bt.bookmark_id HAVING COUNT(DISTINCT LOWER(t.name)) = ?)"
        ));
        for t in &params.tags_and {
            binds.push(ListBind::S(t.to_lowercase()));
        }
        binds.push(ListBind::I(params.tags_and.len() as i64));
    }
    if !params.tags_or.is_empty() {
        let placeholders = params
            .tags_or
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        where_clauses.push(format!(
            "b.id IN (SELECT bt.bookmark_id FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id WHERE LOWER(t.name) IN ({placeholders}))"
        ));
        for t in &params.tags_or {
            binds.push(ListBind::S(t.to_lowercase()));
        }
    }
    if !params.tags_not.is_empty() {
        let placeholders = params
            .tags_not
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        where_clauses.push(format!(
            "b.id NOT IN (SELECT bt.bookmark_id FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id WHERE LOWER(t.name) IN ({placeholders}))"
        ));
        for t in &params.tags_not {
            binds.push(ListBind::S(t.to_lowercase()));
        }
    }
    if let Some(shared) = params.shared {
        where_clauses.push(format!(
            "b.is_shared = {}",
            if shared { "TRUE" } else { "FALSE" }
        ));
    }
    if let Some(unread) = params.unread {
        where_clauses.push(format!(
            "b.is_unread = {}",
            if unread { "TRUE" } else { "FALSE" }
        ));
    }
    // 搜索：优先 AST 表达式；q 仅作纯关键词兜底。
    if let Some(expr) = &params.expr {
        let mut expr_binds: Vec<String> = Vec::new();
        let sql =
            crate::bookmarks::search::compile_lax(expr, &mut expr_binds, params.lax_tag_search);
        where_clauses.push(sql);
        for b in expr_binds {
            binds.push(ListBind::S(b));
        }
    } else if let Some(q) = params.q.as_deref().filter(|s| !s.trim().is_empty()) {
        // q 兜底与表达式同语义：四列 icontains（对齐上游），不再走 FTS 整词匹配。
        where_clauses.push(
            "(LOWER(b.title) LIKE ? OR LOWER(b.description) LIKE ? \
              OR LOWER(b.notes) LIKE ? OR LOWER(b.url) LIKE ?)"
                .into(),
        );
        let pattern = format!(
            "%{}%",
            q.trim()
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
                .to_lowercase()
        );
        for _ in 0..4 {
            binds.push(ListBind::S(pattern.clone()));
        }
    }
    // 时间过滤（对齐上游 modified_since/added_since）：RFC3339 解析失败则静默忽略。
    if let Some(since) = params
        .modified_since
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    {
        where_clauses.push("b.updated_at > ?".into());
        binds.push(ListBind::S(since.to_rfc3339()));
    }
    if let Some(since) = params
        .added_since
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    {
        where_clauses.push("b.created_at > ?".into());
        binds.push(ListBind::S(since.to_rfc3339()));
    }
    (where_clauses.join(" AND "), binds)
}

/// 把构建好的绑定值按序附加到查询（user_id 由调用方先绑定）。
pub(crate) fn attach_binds(
    mut query: crate::db::DbQuery,
    binds: &[ListBind],
) -> crate::db::DbQuery {
    for b in binds {
        query = match b {
            ListBind::S(s) => query.bind(s.as_str()),
            ListBind::I(i) => query.bind(*i),
        };
    }
    query
}

/// 分页列表（含状态/标签/关键词筛选）。
pub async fn list(
    pool: &DbPool,
    user_id: i64,
    params: &ListParams,
) -> Result<(Vec<Bookmark>, i64), AppError> {
    let (where_sql, binds) = build_list_filters(params);
    let order_sql = match params.sort.as_deref() {
        Some("added_asc") => "ORDER BY b.created_at ASC",
        Some("added_desc") | Some("created") => "ORDER BY b.created_at DESC",
        Some("modified_asc") => "ORDER BY b.updated_at ASC",
        Some("modified_desc") | Some("updated") => "ORDER BY b.updated_at DESC",
        Some("title_asc") | Some("title") => "ORDER BY LOWER(b.title) ASC",
        Some("title_desc") => "ORDER BY LOWER(b.title) DESC",
        _ => "ORDER BY b.created_at DESC",
    };

    // 总数。
    let count_sql = format!("SELECT COUNT(*) FROM bookmarks b WHERE {where_sql}");
    let count_row = attach_binds(crate::db::q(&count_sql).bind(user_id), &binds)
        .fetch_one(pool)
        .await?;
    let total: i64 = count_row.get_i64(0);

    // 页数据。
    let data_sql = format!(
        "SELECT b.id, b.user_id, b.url, b.url_hash, b.title, b.description, b.notes, b.is_unread, b.is_archived, b.is_shared,
                b.favicon_path, b.preview_image_path, b.snapshot_path, b.archive_url, b.created_at, b.updated_at
         FROM bookmarks b WHERE {where_sql} {order_sql} LIMIT ? OFFSET ?"
    );
    let data_query = attach_binds(crate::db::q(&data_sql).bind(user_id), &binds)
        .bind(params.effective_per_page())
        .bind(params.offset());
    let rows = data_query.fetch_all(pool).await?;
    let items: Vec<Bookmark> = rows.into_iter().map(row_to_bookmark).collect();
    Ok((items, total))
}

/// 当前过滤条件下书签使用的标签集合（标签云数据源，对齐 linkding query_bookmark_tags）。
/// 返回 (全部标签, 查询中显式指定的标签名集合)。
pub async fn list_tags_for_query(
    pool: &DbPool,
    user_id: i64,
    params: &ListParams,
) -> Result<Vec<(String, i64)>, AppError> {
    let (where_sql, mut binds) = build_list_filters(params);
    let sql = format!(
        "SELECT t.name, COUNT(DISTINCT bt.bookmark_id) AS cnt
         FROM tags t
         JOIN bookmark_tags bt ON bt.tag_id = t.id
         JOIN bookmarks b ON b.id = bt.bookmark_id
         WHERE t.user_id = ? AND {where_sql}
         GROUP BY t.id, t.name"
    );
    // where_sql 首个子句是 b.user_id = ?：插入到绑定序列最前。
    binds.insert(0, ListBind::I(user_id));
    let query = attach_binds(crate::db::q(&sql).bind(user_id), &binds);
    let rows = query.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get_named_string("name"), r.get_named_i64("cnt")))
        .collect())
}

pub(crate) fn row_to_bookmark(r: crate::db::DbRow) -> Bookmark {
    Bookmark {
        id: r.get_named_i64("id"),
        user_id: r.get_named_i64("user_id"),
        url: r.get_named_string("url"),
        url_hash: r.get_named_string("url_hash"),
        title: r.get_named_string("title"),
        description: r.get_named_string("description"),
        notes: r.get_named_string("notes"),
        is_unread: r.get_named_bool("is_unread"),
        is_archived: r.get_named_bool("is_archived"),
        is_shared: r.get_named_bool("is_shared"),
        favicon_path: r.get_named_opt_string("favicon_path"),
        preview_image_path: r.get_named_opt_string("preview_image_path"),
        snapshot_path: r.get_named_opt_string("snapshot_path"),
        archive_url: r.get_named_opt_string("archive_url"),
        created_at: r.get_named_string("created_at"),
        updated_at: r.get_named_string("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup_user() -> (DbPool, i64) {
        let pool = db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        (pool, u.id)
    }

    #[tokio::test]
    async fn create_and_get() {
        let (pool, uid) = setup_user().await;
        let b = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://example.com".into(),
                title: Some("Example".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: true,
                is_shared: false,
                tags: vec!["rust".into()],
            },
        )
        .await
        .unwrap();
        assert!(b.id > 0);
        assert!(b.is_unread);
        let got = get(&pool, uid, b.id).await.unwrap().unwrap();
        assert_eq!(got.url, "https://example.com/");
    }

    #[tokio::test]
    async fn duplicate_url_conflicts() {
        let (pool, uid) = setup_user().await;
        let req = NewBookmark {
            url: "https://example.com".into(),
            title: Some(String::new()),
            description: Some(String::new()),
            notes: Some(String::new()),
            is_unread: false,
            is_shared: false,
            tags: vec![],
        };
        create(&pool, uid, &req).await.unwrap();
        let err = create(&pool, uid, &req).await;
        assert!(matches!(err, Err(AppError::Conflict(_))));
    }

    #[tokio::test]
    async fn duplicate_url_across_users_allowed() {
        let (pool, uid) = setup_user().await;
        let u2 = crate::models::create_user(&pool, "u2", "pw12345", false)
            .await
            .unwrap();
        let req = NewBookmark {
            url: "https://example.com".into(),
            title: Some(String::new()),
            description: Some(String::new()),
            notes: Some(String::new()),
            is_unread: false,
            is_shared: false,
            tags: vec![],
        };
        create(&pool, uid, &req).await.unwrap();
        // 另一用户同一 URL 不冲突。
        assert!(create(&pool, u2.id, &req).await.is_ok());
    }

    #[tokio::test]
    async fn update_changes_fields() {
        let (pool, uid) = setup_user().await;
        let b = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://a.com".into(),
                title: Some("A".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: false,
                is_shared: false,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        let updated = update(
            &pool,
            uid,
            b.id,
            &UpdateBookmark {
                title: Some("New Title".into()),
                is_unread: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(updated.title, "New Title");
        assert!(updated.is_unread);
    }

    #[tokio::test]
    async fn delete_removes() {
        let (pool, uid) = setup_user().await;
        let b = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://a.com".into(),
                title: Some(String::new()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: false,
                is_shared: false,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        assert!(delete(&pool, uid, b.id).await.unwrap());
        assert!(get(&pool, uid, b.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_filters_unread_and_archived() {
        let (pool, uid) = setup_user().await;
        let b1 = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://a.com".into(),
                title: Some("A".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: true,
                is_shared: false,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        let b2 = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://b.com".into(),
                title: Some("B".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: false,
                is_shared: false,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        set_archived(&pool, uid, b2.id, true).await.unwrap();

        let (unread, _) = list(
            &pool,
            uid,
            &ListParams {
                filter: Some("unread".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].id, b1.id);

        let (archived, _) = list(
            &pool,
            uid,
            &ListParams {
                filter: Some("archived".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].id, b2.id);

        let (active, total) = list(&pool, uid, &ListParams::default()).await.unwrap();
        assert_eq!(total, 1);
        assert_eq!(active[0].id, b1.id);
    }

    #[tokio::test]
    async fn list_filters_by_tag() {
        let (pool, uid) = setup_user().await;
        create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://a.com".into(),
                title: Some("A".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: false,
                is_shared: false,
                tags: vec!["rust".into(), "web".into()],
            },
        )
        .await
        .unwrap();
        create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://b.com".into(),
                title: Some("B".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: false,
                is_shared: false,
                tags: vec!["rust".into()],
            },
        )
        .await
        .unwrap();

        let (both, _) = list(
            &pool,
            uid,
            &ListParams {
                tags_and: vec!["rust".into(), "web".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(both.len(), 1);

        let (either, _) = list(
            &pool,
            uid,
            &ListParams {
                tags_or: vec!["web".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(either.len(), 1);
    }

    #[tokio::test]
    async fn list_unread_filter_with_tag_query() {
        let (pool, uid) = setup_user().await;
        let mk =
            |url: &'static str, title: &'static str, unread: bool, tags: Vec<String>| NewBookmark {
                url: url.into(),
                title: Some(title.into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: unread,
                is_shared: false,
                tags,
            };
        let b1 = create(
            &pool,
            uid,
            &mk("https://u1.com", "U1", true, vec!["rust".into()]),
        )
        .await
        .unwrap();
        let _b2 = create(
            &pool,
            uid,
            &mk("https://u2.com", "U2", false, vec!["rust".into()]),
        )
        .await
        .unwrap();

        // filter=unread + q=#rust：仅未读且带该标签。
        let (items, total) = list(
            &pool,
            uid,
            &ListParams {
                filter: Some("unread".into()),
                q: Some("#rust".into()),
                expr: Some(crate::bookmarks::search::parse_query("#rust").unwrap()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(total, 1, "unread + #rust 应只命中未读的 rust 书签");
        assert_eq!(items[0].id, b1.id);
    }

    #[tokio::test]
    async fn list_filters_by_since_timestamps() {
        let (pool, uid) = setup_user().await;
        let b_old = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://old.example.com".into(),
                title: Some("Old".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: true,
                is_shared: false,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        let b_new = create(
            &pool,
            uid,
            &NewBookmark {
                url: "https://new.example.com".into(),
                title: Some("New".into()),
                description: Some(String::new()),
                notes: Some(String::new()),
                is_unread: true,
                is_shared: false,
                tags: vec![],
            },
        )
        .await
        .unwrap();
        // 直接改时间戳：old=2020 年，new=2030 年。
        crate::db::q("UPDATE bookmarks SET created_at = ?, updated_at = ? WHERE id = ?")
            .bind("2020-01-01T00:00:00+00:00")
            .bind("2020-01-01T00:00:00+00:00")
            .bind(b_old.id)
            .execute(&pool)
            .await
            .unwrap();
        crate::db::q("UPDATE bookmarks SET created_at = ?, updated_at = ? WHERE id = ?")
            .bind("2030-01-01T00:00:00+00:00")
            .bind("2030-01-01T00:00:00+00:00")
            .bind(b_new.id)
            .execute(&pool)
            .await
            .unwrap();

        // added_since：2025 年之后 → 只剩 new。
        let (items, total) = list(
            &pool,
            uid,
            &ListParams {
                added_since: Some("2025-06-01T00:00:00+00:00".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, b_new.id);

        // modified_since 同理。
        let (items, total) = list(
            &pool,
            uid,
            &ListParams {
                modified_since: Some("2025-06-01T00:00:00+00:00".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(total, 1);
        assert_eq!(items[0].id, b_new.id);

        // 非法时间戳：静默忽略，返回全部。
        let (_, total) = list(
            &pool,
            uid,
            &ListParams {
                added_since: Some("not-a-date".into()),
                modified_since: Some("also-bad".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(total, 2);

        // 早于全部的时间戳：返回全部。
        let (_, total) = list(
            &pool,
            uid,
            &ListParams {
                added_since: Some("2019-01-01T00:00:00+00:00".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(total, 2);
    }
}
