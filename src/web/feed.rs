use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::bookmarks::crud::{self, ListParams};
use crate::error::AppError;
use crate::web::auth_api::AppState;

/// Feed 查询参数（对齐上游 BaseBookmarksFeed.get_object）。
#[derive(Debug, Deserialize)]
pub struct FeedQuery {
    #[serde(default)]
    pub key: String,
    /// 条数上限；空串 = 全部（对齐上游 `?limit=`）。解析失败时用默认 100。
    pub limit: Option<String>,
    /// 搜索表达式或关键词（支持 #标签 / AND / OR / NOT）。
    #[serde(default)]
    pub q: Option<String>,
    /// shared/public_shared 源：按共享者用户名筛选；未知用户 → 空列表。
    #[serde(default)]
    pub user: Option<String>,
    /// yes/no 过滤未读（all/unread 源）。
    #[serde(default)]
    pub unread: Option<String>,
    /// yes/no 过滤共享（all/unread 源）。
    #[serde(default)]
    pub shared: Option<String>,
    /// 按本人 Bundle（保存的过滤器）过滤；非本人/不存在/非数字 → 404。
    pub bundle: Option<String>,
}

/// kind → (标题, 描述)，对齐上游四个 Feed 子类。
fn feed_meta(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "all" => Some(("All bookmarks", "All bookmarks")),
        "unread" => Some(("Unread bookmarks", "All unread bookmarks")),
        "shared" => Some(("Shared bookmarks", "All shared bookmarks")),
        "public_shared" => Some(("Public shared bookmarks", "All public shared bookmarks")),
        _ => None,
    }
}

/// 校验 feed key → 返回 user_id；缺失/无效 key → 404（对齐上游 Http404）。
async fn authenticate(state: &AppState, key: &str) -> Result<i64, AppError> {
    if key.is_empty() {
        return Err(AppError::NotFound);
    }
    let user_id: Option<i64> =
        crate::db::q_scalar::<()>("SELECT user_id FROM feed_tokens WHERE key = ?")
            .bind(key)
            .fetch_optional(&state.pool)
            .await?
            .map(|r| r.get_i64(0));
    user_id.ok_or(AppError::NotFound)
}

fn yes_no(v: &Option<String>) -> Option<bool> {
    match v.as_deref() {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        _ => None,
    }
}

/// 解析 limit：默认 100；空串 = 全部（用大上限表示）；无效值回退默认。
fn parse_limit(v: &Option<String>) -> i64 {
    match v.as_deref().map(str::trim) {
        None => 100,
        Some("") => 10_000,
        Some(s) => s.parse::<i64>().map(|n| n.clamp(0, 10_000)).unwrap_or(100),
    }
}

/// bundle 参数解析：缺失/空 → None；非数字 → 404（对齐上游 Http404）。
fn parse_bundle_id(v: &Option<String>) -> Result<Option<i64>, AppError> {
    match v.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<i64>().map(Some).map_err(|_| AppError::NotFound),
    }
}

/// `GET /feed/{kind}?key=...`：旧路径（兼容保留）。
/// kind: all / unread / shared / public_shared。
pub async fn feed(
    State(state): State<AppState>,
    Path(kind): Path<String>,
    Query(q): Query<FeedQuery>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, AppError> {
    render_feed(&state, &kind, &q.key, &q, &headers, &uri).await
}

/// `GET /feeds/{key}/{kind}`：对齐上游 `feeds/<key>/<kind>` 路径。
pub async fn feed_keyed(
    State(state): State<AppState>,
    Path((key, kind)): Path<(String, String)>,
    Query(q): Query<FeedQuery>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, AppError> {
    render_feed(&state, &kind, &key, &q, &headers, &uri).await
}

/// `GET /feeds/shared`：公开共享源（匿名可访问，无需 key）。
pub async fn feed_public_shared(
    State(state): State<AppState>,
    Query(q): Query<FeedQuery>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<Response, AppError> {
    render_feed(&state, "public_shared", "", &q, &headers, &uri).await
}

async fn render_feed(
    state: &AppState,
    kind: &str,
    key: &str,
    q: &FeedQuery,
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<Response, AppError> {
    let (title, description) = feed_meta(kind).ok_or(AppError::NotFound)?;

    // public_shared 无需 key；其余 kind 需有效 feed key（无效 → 404）。
    let _viewer: Option<i64> = if kind == "public_shared" {
        None
    } else {
        Some(authenticate(state, key).await?)
    };

    let items = match kind {
        "all" | "unread" => owned_items(state, kind, key, q).await?,
        "shared" | "public_shared" => shared_items(state, kind, q).await?,
        _ => return Err(AppError::NotFound),
    };

    // channel link / atom:link self = 当前 feed 的绝对地址。
    let base = crate::web::base_url_from_headers(headers);
    let feed_url = match uri.query() {
        Some(qs) if !qs.is_empty() => format!("{base}{}?{qs}", uri.path()),
        _ => format!("{base}{}", uri.path()),
    };

    let xml = render_rss(state, title, description, &feed_url, &items).await?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/rss+xml; charset=utf-8")],
        xml,
    )
        .into_response())
}

/// all/unread 源：本人书签（排除归档），支持 q/unread/shared/bundle/limit。
async fn owned_items(
    state: &AppState,
    kind: &str,
    key: &str,
    q: &FeedQuery,
) -> Result<Vec<crud::Bookmark>, AppError> {
    let user_id = authenticate(state, key).await?;
    // all 源排除归档（对齐上游 query_bookmarks 的 is_archived=False）；unread 源自带排除。
    let mut params = ListParams {
        filter: Some(if kind == "unread" { "unread" } else { "active" }.into()),
        ..ListParams::default()
    };

    // Bundle（保存的过滤器）：仅本人 bundle；否则 404。
    let mut bundle_search = crate::bookmarks::bundle::BundleSearch::default();
    if let Some(bid) = parse_bundle_id(&q.bundle)? {
        match crate::bookmarks::bundle::get(&state.pool, user_id, bid).await? {
            Some(b) => {
                bundle_search = serde_json::from_str(&b.search_json).map_err(AppError::internal)?;
            }
            None => return Err(AppError::NotFound),
        }
    }
    params.tags_and = bundle_search.tags_and;
    params.tags_or = bundle_search.tags_or;
    params.tags_not = bundle_search.tags_not;
    params.unread = yes_no(&q.unread);
    params.shared = yes_no(&q.shared);

    // q：URL 参数优先，否则用 bundle 保存的 q。表达式优先，解析失败按关键词兜底。
    let q_str =
        q.q.clone()
            .or(bundle_search.q)
            .filter(|s| !s.trim().is_empty());
    if let Some(s) = &q_str {
        match crate::bookmarks::search::parse_query(s) {
            Ok(expr) => params.expr = Some(expr),
            Err(_) => params.q = Some(s.clone()),
        }
    }
    params.per_page = parse_limit(&q.limit);
    params.page = 1;

    let (items, _) = crud::list(&state.pool, user_id, &params).await?;
    Ok(items)
}

/// shared/public_shared 源：所有开启共享用户的共享书签（对齐上游 query_shared_bookmarks）。
/// shared：is_shared=1 且所有者 enable_sharing=1；public_shared 另加 enable_public_sharing=1。
async fn shared_items(
    state: &AppState,
    kind: &str,
    q: &FeedQuery,
) -> Result<Vec<crud::Bookmark>, AppError> {
    let public = kind == "public_shared";
    // 布尔比较统一 TRUE/FALSE 字面量（PG BOOLEAN 不能比整数）。
    let mut conds: Vec<String> = vec![
        "b.is_shared = TRUE".into(),
        "up.enable_sharing = TRUE".into(),
    ];
    if public {
        conds.push("up.enable_public_sharing = TRUE".into());
    }
    let mut binds: Vec<String> = Vec::new();
    // ?user= 按用户名筛选；未知用户自然无匹配（空列表，对齐上游 none queryset）。
    if let Some(u) = q.user.as_deref().filter(|s| !s.trim().is_empty()) {
        conds.push("u.username = ?".into());
        binds.push(u.to_string());
    }
    // q：表达式优先（#标签/AND/OR/NOT），解析失败按关键词 icontains（绑定参数，防注入）。
    let q_str = q.q.as_deref().filter(|s| !s.trim().is_empty());
    let mut expr_binds: Vec<String> = Vec::new();
    let mut has_expr = false;
    if let Some(s) = q_str
        && let Ok(expr) = crate::bookmarks::search::parse_query(s)
    {
        conds.push(crate::bookmarks::search::compile(&expr, &mut expr_binds));
        has_expr = true;
    }
    if !has_expr && let Some(s) = q_str {
        // 关键词兜底：四列 icontains（LOWER(col) LIKE ?，三库通用；模式经转义与小写化）。
        let like = format!(
            "%{}%",
            s.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
                .to_lowercase()
        );
        conds.push(
            "(LOWER(b.title) LIKE ? OR LOWER(b.url) LIKE ? OR LOWER(b.description) LIKE ? \
              OR LOWER(b.notes) LIKE ?)"
                .into(),
        );
        for _ in 0..4 {
            expr_binds.push(like.clone());
        }
    }

    let where_sql = conds.join(" AND ");
    let sql = format!(
        "SELECT b.id, b.user_id, b.url, b.url_hash, b.title, b.description, b.notes,
                b.is_unread, b.is_archived, b.is_shared, b.favicon_path, b.preview_image_path,
                b.snapshot_path, b.archive_url, b.created_at, b.updated_at
         FROM bookmarks b
         JOIN users u ON u.id = b.user_id
         JOIN user_profiles up ON up.user_id = b.user_id
         WHERE {where_sql}
         ORDER BY b.created_at DESC LIMIT ?"
    );
    let mut query = crate::db::q(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    for b in &expr_binds {
        query = query.bind(b);
    }
    query = query.bind(parse_limit(&q.limit));
    let items = query
        .fetch_all(&state.pool)
        .await?
        .into_iter()
        .map(crud::row_to_bookmark)
        .collect::<Vec<_>>();
    Ok(items)
}

/// 去 Unicode 控制字符（保留 \n \r \t），对齐上游 sanitize。
/// 注：`char::is_control()` 覆盖 Cc 类；上游另剔除 Cf 等罕见不可见字符，此处按常见 Cc 处理。
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect()
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// RFC3339 → RFC2822（对齐上游 item_pubdate 输出）；解析失败回退原串。
fn rfc2822(created_at: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .map(|dt| dt.to_rfc2822())
        .unwrap_or_else(|_| created_at.to_string())
}

async fn render_rss(
    state: &AppState,
    title: &str,
    description: &str,
    feed_url: &str,
    items: &[crud::Bookmark],
) -> Result<String, AppError> {
    let mut body = String::new();
    for b in items {
        let tags = crate::bookmarks::tags::tags_for_bookmark(&state.pool, b.id)
            .await
            .unwrap_or_default();
        let t = sanitize(&b.title);
        let d = sanitize(&b.description);
        body.push_str("    <item>\n");
        body.push_str(&format!("      <title>{}</title>\n", xml_escape(&t)));
        body.push_str(&format!("      <link>{}</link>\n", xml_escape(&b.url)));
        if d.is_empty() {
            body.push_str("      <description/>\n");
        } else {
            body.push_str(&format!(
                "      <description>{}</description>\n",
                xml_escape(&d)
            ));
        }
        body.push_str(&format!(
            "      <pubDate>{}</pubDate>\n",
            xml_escape(&rfc2822(&b.created_at))
        ));
        // guid = 书签 URL（对齐上游 item_link）。
        body.push_str(&format!("      <guid>{}</guid>\n", xml_escape(&b.url)));
        for tag in &tags {
            body.push_str(&format!("      <category>{}</category>\n", xml_escape(tag)));
        }
        body.push_str("    </item>\n");
    }
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>{}</title>
    <link>{}</link>
    <description>{}</description>
    <atom:link href="{}" rel="self"/>
{}
  </channel>
</rss>
"#,
        xml_escape(title),
        xml_escape(feed_url),
        xml_escape(description),
        xml_escape(feed_url),
        body
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_whitespace_and_strips_control_chars() {
        assert_eq!(sanitize("test\n\r\t\x00\x08title"), "test\n\r\ttitle");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn parse_limit_defaults_and_overrides() {
        assert_eq!(parse_limit(&None), 100);
        assert_eq!(parse_limit(&Some("".into())), 10_000);
        assert_eq!(parse_limit(&Some("5".into())), 5);
        assert_eq!(parse_limit(&Some("200".into())), 200);
        assert_eq!(parse_limit(&Some("abc".into())), 100);
        assert_eq!(parse_limit(&Some("-3".into())), 0);
    }

    #[test]
    fn parse_bundle_id_rejects_non_numeric() {
        assert_eq!(parse_bundle_id(&None).unwrap(), None);
        assert_eq!(parse_bundle_id(&Some("".into())).unwrap(), None);
        assert_eq!(parse_bundle_id(&Some("12".into())).unwrap(), Some(12));
        assert!(parse_bundle_id(&Some("invalid".into())).is_err());
    }
}
