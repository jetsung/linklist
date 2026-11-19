use crate::db::DbPool;

use crate::error::AppError;

/// 获取或创建标签，返回标签 id（事务内使用）。
pub async fn get_or_create_tag(
    tx: &mut crate::db::DbTx,
    user_id: i64,
    name: &str,
) -> Result<i64, AppError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("标签名不能为空".into()));
    }
    if let Some(id) =
        crate::db::q_scalar::<()>("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
            .bind(user_id)
            .bind(name.to_lowercase())
            .fetch_optional_tx(tx)
            .await?
            .map(|r| r.get_i64(0))
    {
        return Ok(id);
    }
    // PG 无函数式自增回读，用 RETURNING id；SQLite/MySQL 用响应自带回读。
    let insert = match tx.dialect() {
        crate::db::DbDialect::Postgres => {
            crate::db::q("INSERT INTO tags (user_id, name) VALUES (?, ?) RETURNING id")
        }
        _ => crate::db::q("INSERT INTO tags (user_id, name) VALUES (?, ?)"),
    }
    .bind(user_id)
    .bind(&name);
    let id = match tx {
        crate::db::DbTx::Postgres(_) => insert.fetch_one_tx(tx).await?.get_i64(0),
        // SQLite 与 MySQL 同形态：事务内 INSERT 后响应自带自增回读。
        crate::db::DbTx::Sqlite(_) | crate::db::DbTx::MySql(_) => {
            insert.execute_tx(tx).await?.last_insert_rowid()
        }
    };
    Ok(id)
}

/// 标签排序：英文（A-Z，不区分大小写）在前，中文按拼音（A-Z）在后。
pub fn sort_tags_english_first(tags: &mut [TagWithCount]) {
    use pinyin::ToPinyin;
    use std::cmp::Ordering;

    fn sort_key(name: &str) -> (bool, String) {
        // key.0: false = 英文/ASCII 字母组，true = 中文等非 ASCII 组。
        let first = name.chars().next().unwrap_or('\u{7F}');
        let is_ascii_alpha = first.is_ascii_alphabetic();
        let pinyin_key: String = if is_ascii_alpha {
            name.to_lowercase()
        } else {
            name.chars()
                .flat_map(|c| c.to_pinyin().map(|p| p.plain().to_string()))
                .collect::<Vec<_>>()
                .join("")
                .to_lowercase()
        };
        (!is_ascii_alpha, pinyin_key)
    }

    tags.sort_by(|a, b| {
        let (ka, kb) = (sort_key(&a.name), sort_key(&b.name));
        match ka.0.cmp(&kb.0) {
            Ordering::Equal => ka.1.cmp(&kb.1).then_with(|| a.name.cmp(&b.name)),
            o => o,
        }
    });
}

/// 用户全部标签及使用计数（英文在前，中文拼音序在后）。
pub async fn list_tags(pool: &DbPool, user_id: i64) -> Result<Vec<TagWithCount>, AppError> {
    let rows = crate::db::q(
        "SELECT t.id, t.name, COUNT(bt.bookmark_id) AS usage_count
         FROM tags t LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
         WHERE t.user_id = ?
         GROUP BY t.id, t.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    let mut tags: Vec<TagWithCount> = rows
        .into_iter()
        .map(|r| TagWithCount {
            id: r.get_named_i64("id"),
            name: r.get_named_string("name"),
            usage_count: r.get_named_i64("usage_count"),
        })
        .collect();
    sort_tags_english_first(&mut tags);
    Ok(tags)
}

/// 前缀自动补全（按使用频率排序）。
pub async fn complete_tags(
    pool: &DbPool,
    user_id: i64,
    prefix: &str,
    limit: i64,
) -> Result<Vec<TagWithCount>, AppError> {
    let pattern = format!("{}%", prefix.to_ascii_lowercase());
    let rows = crate::db::q(
        "SELECT t.id, t.name, COUNT(bt.bookmark_id) AS usage_count
         FROM tags t LEFT JOIN bookmark_tags bt ON bt.tag_id = t.id
         WHERE t.user_id = ? AND LOWER(t.name) LIKE ?
         GROUP BY t.id, t.name
         ORDER BY usage_count DESC, LOWER(t.name) ASC
         LIMIT ?",
    )
    .bind(user_id)
    .bind(pattern)
    .bind(limit.clamp(1, 50))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TagWithCount {
            id: r.get_named_i64("id"),
            name: r.get_named_string("name"),
            usage_count: r.get_named_i64("usage_count"),
        })
        .collect())
}

/// 设置书签的标签集合（整体替换）。
pub async fn set_bookmark_tags(
    pool: &DbPool,
    user_id: i64,
    bookmark_id: i64,
    tags: &[String],
) -> Result<(), AppError> {
    // 校验书签归属。
    let owner: Option<i64> =
        crate::db::q_scalar::<()>("SELECT user_id FROM bookmarks WHERE id = ?")
            .bind(bookmark_id)
            .fetch_optional(pool)
            .await?
            .map(|r| r.get_i64(0));
    if owner != Some(user_id) {
        return Err(AppError::NotFound);
    }
    let mut tx = pool.begin().await?;
    crate::db::q("DELETE FROM bookmark_tags WHERE bookmark_id = ?")
        .bind(bookmark_id)
        .execute_tx(&mut tx)
        .await?;
    for name in tags {
        let tag_id = get_or_create_tag(&mut tx, user_id, name).await?;
        crate::db::q(crate::db::sql_insert_ignore(
            tx.dialect(),
            "bookmark_tags (bookmark_id, tag_id)",
            "?, ?",
        ))
        .bind(bookmark_id)
        .bind(tag_id)
        .execute_tx(&mut tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 书签的标签列表。
pub async fn tags_for_bookmark(pool: &DbPool, bookmark_id: i64) -> Result<Vec<String>, AppError> {
    let rows = crate::db::q(
        "SELECT t.name FROM tags t JOIN bookmark_tags bt ON bt.tag_id = t.id WHERE bt.bookmark_id = ? ORDER BY t.name",
    )
    .bind(bookmark_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| r.get_named_string("name"))
        .collect())
}

/// 标签及使用计数。
#[derive(Debug, serde::Serialize)]
pub struct TagWithCount {
    pub id: i64,
    pub name: String,
    pub usage_count: i64,
}

/// 删除当前用户的指定标签及其书签关联。
pub async fn delete_tag(pool: &DbPool, user_id: i64, name: &str) -> Result<bool, AppError> {
    let mut tx = pool.begin().await?;
    let tag_id: Option<i64> =
        crate::db::q_scalar::<()>("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
            .bind(user_id)
            .bind(name.to_lowercase())
            .fetch_optional_tx(&mut tx)
            .await?
            .map(|r| r.get_i64(0));
    let Some(tag_id) = tag_id else {
        return Ok(false);
    };
    crate::db::q("DELETE FROM bookmark_tags WHERE tag_id = ?")
        .bind(tag_id)
        .execute_tx(&mut tx)
        .await?;
    crate::db::q("DELETE FROM tags WHERE id = ?")
        .bind(tag_id)
        .execute_tx(&mut tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// 重命名标签；若新名已存在则报冲突。
pub async fn rename_tag(
    pool: &DbPool,
    user_id: i64,
    name: &str,
    new_name: &str,
) -> Result<(), AppError> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(AppError::BadRequest("标签名不能为空".into()));
    }
    let tag_id: Option<i64> =
        crate::db::q_scalar::<()>("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
            .bind(user_id)
            .bind(name.to_lowercase())
            .fetch_optional(pool)
            .await?
            .map(|r| r.get_i64(0));
    let Some(tag_id) = tag_id else {
        return Err(AppError::NotFound);
    };
    let exists: Option<i64> = crate::db::q_scalar::<()>(
        "SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ? AND id != ?",
    )
    .bind(user_id)
    .bind(new_name.to_lowercase())
    .bind(tag_id)
    .fetch_optional(pool)
    .await?
    .map(|r| r.get_i64(0));
    if exists.is_some() {
        return Err(AppError::BadRequest(format!("标签「{new_name}」已存在")));
    }
    crate::db::q("UPDATE tags SET name = ? WHERE id = ?")
        .bind(new_name)
        .bind(tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 合并标签：把 sources 的所有书签关联转移到 target（不存在则创建），删除 sources。
/// 对齐 linkding TagMergeForm：目标标签必须已存在；待合并标签不能包含目标标签。
pub async fn merge_tags_multi(
    pool: &DbPool,
    user_id: i64,
    target: &str,
    sources: &[String],
) -> Result<(), AppError> {
    let target = target.trim();
    if target.is_empty() {
        return Err(AppError::BadRequest("目标标签不能为空".into()));
    }
    let names: Vec<&str> = sources
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return Err(AppError::BadRequest("请输入至少一个要合并的标签".into()));
    }
    if names.iter().any(|s| s.eq_ignore_ascii_case(target)) {
        return Err(AppError::BadRequest(
            "目标标签不能同时出现在要合并的标签里".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let tgt_id: Option<i64> =
        crate::db::q_scalar::<()>("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
            .bind(user_id)
            .bind(target.to_lowercase())
            .fetch_optional_tx(&mut tx)
            .await?
            .map(|r| r.get_i64(0));
    let Some(tgt_id) = tgt_id else {
        return Err(AppError::BadRequest(format!("目标标签「{target}」不存在")));
    };
    let mut src_ids: Vec<i64> = Vec::new();
    for name in &names {
        let id: Option<i64> =
            crate::db::q_scalar::<()>("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
                .bind(user_id)
                .bind(name.to_lowercase())
                .fetch_optional_tx(&mut tx)
                .await?
                .map(|r| r.get_i64(0));
        match id {
            Some(id) if id != tgt_id => src_ids.push(id),
            Some(_) => {
                return Err(AppError::BadRequest(
                    "目标标签不能同时出现在要合并的标签里".into(),
                ));
            }
            None => return Err(AppError::BadRequest(format!("标签「{name}」不存在"))),
        }
    }
    // 迁移关联：目标已存在的关联先删除，避免唯一约束冲突；再并入目标。
    for src_id in &src_ids {
        crate::db::q(
            "DELETE FROM bookmark_tags WHERE tag_id = ? AND bookmark_id IN (SELECT bookmark_id FROM bookmark_tags WHERE tag_id = ?)",
        )
        .bind(tgt_id)
        .bind(*src_id)
        .execute_tx(&mut tx)
        .await?;
        crate::db::q("UPDATE bookmark_tags SET tag_id = ? WHERE tag_id = ?")
            .bind(tgt_id)
            .bind(*src_id)
            .execute_tx(&mut tx)
            .await?;
    }
    for id in &src_ids {
        crate::db::q("DELETE FROM tags WHERE id = ?")
            .bind(*id)
            .execute_tx(&mut tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_puts_english_first_then_chinese_by_pinyin() {
        let mut tags = vec![
            tag("翻译"),
            tag("GitHub"),
            tag("工具"),
            tag("AI"),
            tag("百度"),
            tag("AI"), // 重复名不影响稳定性验证
        ];
        sort_tags_english_first(&mut tags);
        let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["AI", "AI", "GitHub", "百度", "翻译", "工具"]);
    }

    /// TODO7 回归：标签大小写不敏感（iexact）。
    #[tokio::test]
    async fn get_or_create_tag_is_case_insensitive() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw123456", false)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let id1 = get_or_create_tag(&mut tx, u.id, "Rust").await.unwrap();
        // 仅大小写不同 → 复用同一标签。
        let id2 = get_or_create_tag(&mut tx, u.id, "rust").await.unwrap();
        let id3 = get_or_create_tag(&mut tx, u.id, "RUST").await.unwrap();
        tx.commit().await.unwrap();
        assert_eq!(id1, id2);
        assert_eq!(id1, id3);
        let count: i64 = crate::db::q_scalar::<()>("SELECT COUNT(*) FROM tags")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get_i64(0);
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn rename_tag_rejects_case_insensitive_conflict() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u2", "pw123456", false)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        get_or_create_tag(&mut tx, u.id, "Rust").await.unwrap();
        get_or_create_tag(&mut tx, u.id, "Web").await.unwrap();
        tx.commit().await.unwrap();
        // 重命名为仅大小写不同的名字 → 冲突。
        let err = rename_tag(&pool, u.id, "Web", "RUST").await;
        assert!(err.is_err(), "应报标签已存在冲突");
        // 重命名为全新名字 → 成功。
        rename_tag(&pool, u.id, "Web", "Frontend").await.unwrap();
        let names: Vec<String> = crate::db::q_scalar::<()>("SELECT name FROM tags ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.get_string(0))
            .collect();
        assert_eq!(names, vec!["Rust".to_string(), "Frontend".to_string()]);
    }

    fn tag(name: &str) -> TagWithCount {
        TagWithCount {
            id: 0,
            name: name.into(),
            usage_count: 0,
        }
    }
}
