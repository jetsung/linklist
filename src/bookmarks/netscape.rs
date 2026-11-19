use crate::db::DbPool;
use scraper::{Html, Selector};
use serde::Serialize;

use crate::bookmarks::crud::{self, NewBookmark};
use crate::bookmarks::normalize::normalize_url;
use crate::error::AppError;

/// 导入统计。
#[derive(Debug, Serialize)]
pub struct ImportStats {
    pub imported: usize,
    pub duplicates: usize,
    pub failed: usize,
}

/// 单条解析结果。
#[derive(Debug)]
pub struct ParsedBookmark {
    pub url: String,
    pub title: String,
    pub tags: Vec<String>,
    pub description: String,
    pub notes: String,
    pub is_unread: bool,
    /// PRIVATE 属性原始值语义：true=私有（缺省/1），false=公开（0）。
    /// 上游 map_private_flag 需要区分「显式 0」与「缺省」。
    pub private: bool,
    pub is_shared: bool,
    pub is_archived: bool,
    /// Netscape ADD_DATE 属性 → 书签创建时间。
    pub add_date: Option<String>,
    /// Netscape LAST_MODIFIED 属性 → 书签更新时间。
    pub last_modified: Option<String>,
}

/// 解析 Netscape 时间戳（秒）为 RFC3339。
fn parse_netscape_ts(attr: Option<&str>) -> Option<String> {
    attr.and_then(|ts| ts.parse::<i64>().ok())
        .and_then(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0))
        .map(|d| d.to_rfc3339())
}
/// 标签名清洗（对齐 linkding sanitize_tag_name：去首尾空格，内部空格替换为 -）。
fn sanitize_tag_name(name: &str) -> String {
    name.trim().replace(' ', "-")
}

/// 解析 TAGS 属性：逗号分隔、清洗、去空、去重（大小写不敏感）、排序（对齐 linkding parse_tag_string）。
fn parse_tag_string(tag_string: &str) -> Vec<String> {
    if tag_string.trim().is_empty() {
        return Vec::new();
    }
    let mut names: Vec<String> = tag_string
        .split(',')
        .filter_map(|n| {
            let n = sanitize_tag_name(n);
            if n.is_empty() { None } else { Some(n) }
        })
        .collect();
    // 大小写不敏感去重。
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.to_lowercase()));
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// 从 DD 文本提取描述与 notes（对齐 linkding：[linkding-notes]…[/linkding-notes] 为备注）。
fn split_description(text: &str) -> (String, String) {
    if let Some(i) = text.find("[linkding-notes]") {
        let desc = text[..i].trim().to_string();
        let rest = &text[i + "[linkding-notes]".len()..];
        let notes = rest
            .split("[/linkding-notes]")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        (desc, notes)
    } else {
        (text.trim().to_string(), String::new())
    }
}

/// 解析 Netscape HTML 书签文件。
///
/// 支持两类标签来源（合并去重）：`TAGS` 属性（linkding/浏览器导出格式，逗号分隔），
/// 以及 `<H3>` 文件夹名（Netscape 层级结构）。同时识别 `TOREAD` 属性（值为 1 表示未读）、
/// `linkding:bookmarks.archived` 内部标签（表示归档），以及 `<DD>` 描述与
/// `[linkding-notes]…[/linkding-notes]` 备注。
pub fn parse_netscape(html: &str) -> Vec<ParsedBookmark> {
    let doc = Html::parse_document(html);
    let a_selector = Selector::parse("a[href]").unwrap();
    let mut out = Vec::new();

    for a in doc.select(&a_selector) {
        let url = a.value().attr("href").unwrap_or("").trim().to_string();
        if url.is_empty() {
            continue;
        }
        let title = a.text().collect::<String>().trim().to_string();
        let add_date = parse_netscape_ts(a.value().attr("add_date"));
        let last_modified = parse_netscape_ts(a.value().attr("last_modified"));

        // TAGS 属性 → 标签列表；linkding:bookmarks.archived 是内部归档标记。
        let tags_attr = a.value().attr("tags").unwrap_or("");
        let mut tag_names = parse_tag_string(tags_attr);
        let is_archived = tag_names
            .iter()
            .any(|t| t.eq_ignore_ascii_case("linkding:bookmarks.archived"));
        tag_names.retain(|t| !t.eq_ignore_ascii_case("linkding:bookmarks.archived"));

        let toread = a.value().attr("toread").unwrap_or("");
        let is_unread = toread == "1";
        let private = a.value().attr("private").unwrap_or("");
        let is_shared = private == "0";
        let private = private != "0";

        // H3 文件夹名作为补充标签（仅当没有 TAGS 属性时采用，避免重复）。
        if tag_names.is_empty() {
            let mut cursor = a.parent();
            'outer: while let Some(node) = cursor {
                let mut sib = node.prev_sibling();
                while let Some(s) = sib {
                    let is_h3 = s
                        .value()
                        .as_element()
                        .map(|e| e.name() == "h3")
                        .unwrap_or(false);
                    if is_h3 {
                        let text: String = s
                            .descendants()
                            .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                            .collect::<Vec<_>>()
                            .join("");
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            tag_names.push(sanitize_tag_name(&text));
                        }
                        break 'outer;
                    }
                    sib = s.prev_sibling();
                }
                cursor = node.parent();
            }
        }

        // DD 描述：html5ever 会把 <DD> 提升为 A 所在 DT 的兄弟节点；
        // 从 A 的父级（DT）兄弟链找第一个 DD，同时兼容 A 的直接兄弟 DD。
        let mut description = String::new();
        let mut notes = String::new();
        let mut sib = a.parent().and_then(|p| p.next_sibling());
        while let Some(s) = sib {
            if s.value()
                .as_element()
                .map(|e| e.name() == "dd")
                .unwrap_or(false)
            {
                let text: String = s
                    .descendants()
                    .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                    .collect::<Vec<_>>()
                    .join("");
                let (d, n) = split_description(&text);
                description = d;
                notes = n;
                break;
            }
            sib = s.next_sibling();
        }

        out.push(ParsedBookmark {
            url,
            title,
            tags: tag_names,
            description,
            notes,
            is_unread,
            private,
            is_shared,
            is_archived,
            add_date,
            last_modified,
        });
    }
    out
}

/// 导入选项（对齐上游 importer.ImportOptions）。
#[derive(Debug, Default, Clone)]
pub struct ImportOptions {
    /// true 时把 Netscape `PRIVATE="0"` 的书签映射为共享（上游 map_private_flag）。
    /// false 时保持解析默认（PRIVATE=0 → 共享）。
    pub map_private_flag: bool,
}

/// 导入：逐条创建；重复 url_hash 执行覆盖更新（上游 bulk_update 语义），
/// 损坏条目计入 failed。
pub async fn import(
    pool: &DbPool,
    user_id: i64,
    html: &str,
    options: &ImportOptions,
) -> Result<ImportStats, AppError> {
    let parsed = parse_netscape(html);
    let mut stats = ImportStats {
        imported: 0,
        duplicates: 0,
        failed: 0,
    };
    for item in parsed {
        let url = normalize_url(&item.url);
        if url.is_empty() || url == "https:///" {
            stats.failed += 1;
            continue;
        }
        // 上游语义：map_private_flag 且解析结果非私有（PRIVATE=0）→ shared=true。
        let is_shared = if options.map_private_flag && !item.private {
            true
        } else {
            item.is_shared
        };
        let req = NewBookmark {
            url: item.url.clone(),
            title: Some(item.title.clone()),
            description: Some(item.description.clone()),
            notes: Some(item.notes.clone()),
            is_unread: item.is_unread,
            is_shared,
            tags: item.tags.clone(),
        };
        match crud::create(pool, user_id, &req).await {
            Ok(b) => {
                // 回填 Netscape 导出的原始时间（ADD_DATE/LAST_MODIFIED）。
                if item.add_date.is_some() || item.last_modified.is_some() {
                    let created = item
                        .add_date
                        .clone()
                        .unwrap_or_else(|| b.created_at.clone());
                    let updated = item.last_modified.clone().unwrap_or(created.clone());
                    let _ = crate::db::q(
                        "UPDATE bookmarks SET created_at = ?, updated_at = ? WHERE id = ?",
                    )
                    .bind(&created)
                    .bind(&updated)
                    .bind(b.id)
                    .execute(pool)
                    .await;
                }
                if item.is_archived {
                    let _ = crud::update(
                        pool,
                        user_id,
                        b.id,
                        &crud::UpdateBookmark {
                            is_archived: Some(true),
                            ..Default::default()
                        },
                    )
                    .await;
                }
                stats.imported += 1;
            }
            Err(AppError::Conflict(_)) => {
                // 重复书签：覆盖合并（上游 bulk_update 语义）——
                // 标题/描述/notes/未读/共享/归档与日期均以导入内容覆盖；
                // 标签为追加合并，不删除已有标签。
                let hash = crate::bookmarks::normalize::url_hash(&normalize_url(&item.url));
                let existing_id: Option<i64> = crate::db::q_scalar::<()>(
                    "SELECT id FROM bookmarks WHERE user_id = ? AND url_hash = ?",
                )
                .bind(user_id)
                .bind(&hash)
                .fetch_optional(pool)
                .await?
                .map(|r| r.get_i64(0));
                if let Some(bid) = existing_id {
                    let created = item
                        .add_date
                        .clone()
                        .unwrap_or_else(|| item.last_modified.clone().unwrap_or_default());
                    let updated = item
                        .last_modified
                        .clone()
                        .unwrap_or_else(|| created.clone());
                    let _ = crate::db::q(
                        "UPDATE bookmarks SET title = ?, description = ?, notes = ?, is_unread = ?, is_shared = ?, is_archived = ?, created_at = ?, updated_at = ? WHERE id = ? AND user_id = ?",
                    )
                    .bind(if item.title.is_empty() { None } else { Some(item.title.as_str()) })
                    .bind(&item.description)
                    .bind(&item.notes)
                    .bind(item.is_unread)
                    .bind(item.is_shared)
                    .bind(item.is_archived)
                    .bind(&created)
                    .bind(&updated)
                    .bind(bid)
                    .bind(user_id)
                    .execute(pool)
                    .await;
                    // 标签追加合并。
                    for tag_name in &item.tags {
                        let mut tx = match pool.begin().await {
                            Ok(t) => t,
                            Err(_) => break,
                        };
                        if let Ok(tag_id) =
                            crate::bookmarks::tags::get_or_create_tag(&mut tx, user_id, tag_name)
                                .await
                        {
                            let _ = crate::db::q(crate::db::sql_insert_ignore(
                                tx.dialect(),
                                "bookmark_tags (bookmark_id, tag_id)",
                                "?, ?",
                            ))
                            .bind(bid)
                            .bind(tag_id)
                            .execute_tx(&mut tx)
                            .await;
                        }
                        let _ = tx.commit().await;
                    }
                }
                stats.duplicates += 1;
            }
            Err(_) => stats.failed += 1,
        }
    }
    Ok(stats)
}

/// HTML 转义。
fn escape_html(s: &str) -> String {
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

/// Netscape 时间（RFC3339）→ Unix 秒（导出 ADD_DATE/LAST_MODIFIED 属性）。
fn netscape_ts(rfc3339: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .ok()
        .map(|d| d.timestamp())
}

/// 导出为 Netscape HTML（对齐 linkding exporter：全量书签含已归档，
/// 输出 TAGS/ADD_DATE/LAST_MODIFIED/PRIVATE/TOREAD、归档伪标签与 [linkding-notes]）。
pub async fn export(pool: &DbPool, user_id: i64) -> Result<String, AppError> {
    // 全量导出（含已归档），按创建时间升序，对齐上游 settings.export 行为。
    let params = crud::ListParams {
        filter: Some("all".into()),
        sort: Some("added_asc".into()),
        per_page: 200,
        ..Default::default()
    };
    let mut items = Vec::new();
    let mut page = 1i64;
    loop {
        let mut p = params.clone();
        p.page = page;
        let (batch, total) = crud::list(pool, user_id, &p).await?;
        let empty = batch.is_empty();
        let fetched = items.len() as i64 + batch.len() as i64;
        items.extend(batch);
        if fetched >= total || empty {
            break;
        }
        page += 1;
    }
    let mut html = String::new();
    html.push_str("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n");
    html.push_str("<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n");
    html.push_str("<TITLE>Bookmarks</TITLE>\n");
    html.push_str("<H1>Bookmarks</H1>\n");
    html.push_str("<DL><p>\n");
    for b in &items {
        let mut tags = crate::bookmarks::tags::tags_for_bookmark(pool, b.id).await?;
        if b.is_archived {
            tags.push("linkding:bookmarks.archived".into());
        }
        html.push_str("    <DT><A HREF=\"");
        html.push_str(&escape_html(&b.url));
        html.push('"');
        if let Some(ts) = netscape_ts(&b.created_at) {
            html.push_str(&format!(" ADD_DATE=\"{ts}\""));
        }
        if let Some(ts) = netscape_ts(&b.updated_at) {
            html.push_str(&format!(" LAST_MODIFIED=\"{ts}\""));
        }
        // PRIVATE：0=公开（共享），1=私有。
        html.push_str(if b.is_shared {
            " PRIVATE=\"0\""
        } else {
            " PRIVATE=\"1\""
        });
        // TOREAD：1=未读。
        if b.is_unread {
            html.push_str(" TOREAD=\"1\"");
        }
        if !tags.is_empty() {
            html.push_str(" TAGS=\"");
            html.push_str(&escape_html(&tags.join(",")));
            html.push('"');
        }
        html.push('>');
        html.push_str(&escape_html(&b.title));
        html.push_str("</A>\n");
        let mut dd = String::new();
        if !b.description.is_empty() {
            dd.push_str(&b.description);
        }
        if !b.notes.is_empty() {
            if !dd.is_empty() {
                dd.push(' ');
            }
            dd.push_str("[linkding-notes]");
            dd.push_str(&b.notes);
            dd.push_str("[/linkding-notes]");
        }
        if !dd.is_empty() {
            html.push_str("        <DD>");
            html.push_str(&escape_html(&dd));
            html.push('\n');
        }
    }
    html.push_str("</DL><p>\n");
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    const SAMPLE: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
    <DT><H3>开发</H3>
    <DL><p>
        <DT><A HREF="https://www.rust-lang.org/" ADD_DATE="1700000000">Rust 官网</A>
        <DT><A HREF="https://docs.rs/">Docs.rs</A>
    </DL><p>
    <DT><A HREF="https://example.com/" ADD_DATE="1700000100">示例</A>
</DL><p>
"#;

    const BROKEN: &str = r#"<DL><p>
    <DT><A HREF="">空链接</A>
    <DT><A HREF="https://good.com/">有效</A>
    <DT><A>无 href</A>
</DL><p>
"#;

    #[test]
    fn parse_sample() {
        let parsed = parse_netscape(SAMPLE);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].url, "https://www.rust-lang.org/");
        assert_eq!(parsed[0].title, "Rust 官网");
        assert_eq!(parsed[0].tags, vec!["开发"]);
        assert!(parsed[0].add_date.is_some());
        assert_eq!(parsed[2].url, "https://example.com/");
    }

    #[test]
    fn parse_broken_keeps_valid() {
        let parsed = parse_netscape(BROKEN);
        // 无 href 的 A 不进入结果；空 href 也不进入。
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, "https://good.com/");
    }

    const TAGGED: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks</H1>
<DL><p>
<DT><A HREF="https://www.idev.top/" ADD_DATE="1756814411" PRIVATE="0" TOREAD="0" TAGS="我的网站">爱开发</A>
<DD>开发者技术资讯网站
<DT><A HREF="https://github.com/jetsung" TAGS="GitHub,我的源码">GitHub</A>
<DD>（国外）作品项目代码开源
<DT><A HREF="https://filetas.asfd.cn" TAGS="下载,linkding:bookmarks.archived">文件加速下载</A>
<DD>文件加速下载（CF 优速）[linkding-notes]**License**: MIT[/linkding-notes]
</DL><p>
"#;

    #[test]
    fn parse_tags_attribute() {
        let parsed = parse_netscape(TAGGED);
        assert_eq!(parsed.len(), 3);
        // TAGS 属性解析 + 排序去重。
        assert_eq!(parsed[0].tags, vec!["我的网站"]);
        assert_eq!(parsed[1].tags, vec!["GitHub", "我的源码"]);
        // linkding:bookmarks.archived 是内部标记 → 归档而非标签。
        assert_eq!(parsed[2].tags, vec!["下载"]);
        assert!(parsed[2].is_archived);
        assert!(!parsed[0].is_archived);
        // TOREAD / PRIVATE 属性。
        assert!(!parsed[0].is_unread);
        assert!(parsed[0].is_shared, "PRIVATE=0 应映射为共享");
        assert!(!parsed[1].is_shared, "缺省 PRIVATE 按私有处理");
        // DD 描述与 notes。
        assert_eq!(parsed[0].description, "开发者技术资讯网站");
        assert_eq!(parsed[2].description, "文件加速下载（CF 优速）");
        assert_eq!(parsed[2].notes, "**License**: MIT");
        // 日期解析。
        assert_eq!(
            parsed[0].add_date.as_deref(),
            Some("2025-09-02T12:00:11+00:00")
        );
        assert!(parsed[1].add_date.is_none(), "缺省 ADD_DATE 应为 None");
    }

    #[tokio::test]
    async fn import_dedup_and_stats() {
        let pool = db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let stats = import(&pool, u.id, SAMPLE, &ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(stats.imported, 3);
        assert_eq!(stats.duplicates, 0);
        // 重复导入：全部判重。
        let stats2 = import(&pool, u.id, SAMPLE, &ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(stats2.imported, 0);
        assert_eq!(stats2.duplicates, 3);
        // 损坏样例：空 href 与无 href 的 A 在解析阶段被跳过，仅 1 条有效。
        let stats3 = import(&pool, u.id, BROKEN, &ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(stats3.imported, 1);
        assert_eq!(stats3.duplicates, 0);
        assert_eq!(stats3.failed, 0);
    }

    #[tokio::test]
    async fn export_roundtrip() {
        let pool = db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        import(&pool, u.id, SAMPLE, &ImportOptions::default())
            .await
            .unwrap();
        let html = export(&pool, u.id).await.unwrap();
        assert!(html.contains("text/html") || html.contains("NETSCAPE"));
        assert!(html.contains("https://www.rust-lang.org/"));
        assert!(html.contains("TAGS=\"开发\""));
        // round-trip：再导入导出内容，URL 全部保留。
        let pool2 = db::connect_in_memory().await.unwrap();
        let u2 = crate::models::create_user(&pool2, "u2", "pw12345", false)
            .await
            .unwrap();
        import(&pool2, u2.id, &html, &ImportOptions::default())
            .await
            .unwrap();
        let html2 = export(&pool2, u2.id).await.unwrap();
        for url in [
            "https://www.rust-lang.org/",
            "https://docs.rs/",
            "https://example.com/",
        ] {
            assert!(html2.contains(url), "round-trip 应保留 {url}");
        }
    }

    /// TODO5 回归：map_private_flag 语义——开启时仅 PRIVATE=0 的条目强制共享。
    #[tokio::test]
    async fn import_map_private_flag() {
        let pool = db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let html = r#"
<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
  <DT><A HREF="https://a.example/" PRIVATE="0">显式公开</A>
  <DT><A HREF="https://b.example/">缺省私有</A>
  <DT><A HREF="https://c.example/" PRIVATE="1">显式私有</A>
</DL><p>"#;
        // 开启 map_private_flag：PRIVATE=0 → shared=true；缺省/1 → 私有。
        let opts = ImportOptions {
            map_private_flag: true,
        };
        import(&pool, u.id, html, &opts).await.unwrap();
        let shared: i64 = crate::db::q_scalar::<()>(
            "SELECT COUNT(*) FROM bookmarks WHERE user_id = ? AND is_shared = 1",
        )
        .bind(u.id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get_i64(0);
        assert_eq!(shared, 1, "仅 PRIVATE=0 条目应为共享");

        // 关闭 map_private_flag：保持解析默认（仅 PRIVATE=0 共享）。
        let pool2 = db::connect_in_memory().await.unwrap();
        let u2 = crate::models::create_user(&pool2, "u2", "pw12345", false)
            .await
            .unwrap();
        import(&pool2, u2.id, html, &ImportOptions::default())
            .await
            .unwrap();
        let shared2: i64 = crate::db::q_scalar::<()>(
            "SELECT COUNT(*) FROM bookmarks WHERE user_id = ? AND is_shared = 1",
        )
        .bind(u2.id)
        .fetch_one(&pool2)
        .await
        .unwrap()
        .get_i64(0);
        assert_eq!(shared2, 1, "关闭开关时语义不变");
    }

    /// TODO5 回归：重复 URL 导入执行覆盖更新（标题/描述/notes/未读/归档），
    /// 标签追加合并，duplicates 计数。
    #[tokio::test]
    async fn import_duplicate_merges_over_existing() {
        let pool = db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let first = r#"
<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
  <DT><A HREF="https://merge.example/" ADD_DATE="1700000000">旧标题</A>
  <DD>旧描述</DD>
</DL><p>"#;
        import(&pool, u.id, first, &ImportOptions::default())
            .await
            .unwrap();

        let second = r#"
<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
  <DT><A HREF="https://merge.example/" TOREAD="1" ADD_DATE="1700000100" LAST_MODIFIED="1700000200">新标题</A>
  <DD>新描述</DD>
</DL><p>"#;
        let stats = import(&pool, u.id, second, &ImportOptions::default())
            .await
            .unwrap();
        assert_eq!(stats.imported, 0);
        assert_eq!(stats.duplicates, 1, "重复 URL 应计入 duplicates");

        let row = crate::db::q(
            "SELECT title, description, is_unread, created_at, updated_at, (SELECT COUNT(*) FROM bookmark_tags bt JOIN tags t ON t.id=bt.tag_id WHERE bt.bookmark_id=b.id) FROM bookmarks b WHERE user_id = ? AND url = 'https://merge.example/'",
        )
        .bind(u.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get_string(0), "新标题", "标题应被覆盖");
        assert_eq!(row.get_string(1), "新描述", "描述应被覆盖");
        assert_eq!(row.get_i64(2), 1, "TOREAD=1 应覆盖为未读");
        assert!(
            row.get_string(3).contains("2023-11-14"),
            "ADD_DATE 应回填，实际 {}",
            row.get_string(3)
        );
        assert!(
            row.get_string(4).contains("2023-11-14"),
            "LAST_MODIFIED 应回填，实际 {}",
            row.get_string(4)
        );
        assert_eq!(row.get_i64(5), 0);

        // 同 URL 追加标签：应合并而非覆盖。
        let third = r#"
<!DOCTYPE NETSCAPE-Bookmark-file-1>
<DL><p>
  <DT><A HREF="https://merge.example/" TAGS="rust">新标题</A>
</DL><p>"#;
        import(&pool, u.id, third, &ImportOptions::default())
            .await
            .unwrap();
        let tags: i64 = crate::db::q_scalar::<()>(
            "SELECT COUNT(*) FROM bookmark_tags bt JOIN tags t ON t.id = bt.tag_id JOIN bookmarks b ON b.id = bt.bookmark_id WHERE b.user_id = ? AND b.url = 'https://merge.example/'",
        )
        .bind(u.id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .get_i64(0);
        assert_eq!(tags, 1, "标签追加合并");
    }
}
