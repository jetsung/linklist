use crate::db::DbPool;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::bookmarks::crud::ListParams;
use crate::error::AppError;

/// Bundle（保存的过滤器）。
#[derive(Debug, Clone, Serialize)]
pub struct Bundle {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    /// 过滤条件（ListParams 子集）的 JSON，查询时反序列化并应用。
    pub search_json: String,
    pub created_at: String,
    /// 显示顺序（小在前，对齐 linkding order）。
    pub sort_order: i64,
}

/// 过滤条件负载（应用到列表查询的可保存子集）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct BundleSearch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    /// 独立阅读状态过滤（对齐 linkding filter_unread）：Some(true)=仅未读，Some(false)=仅已读。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags_and: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags_or: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags_not: Vec<String>,
    /// Some(true)=仅共享，Some(false)=仅未共享。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewBundle {
    pub name: String,
    #[serde(default)]
    pub search: BundleSearch,
}

/// 创建 bundle。
pub async fn create(pool: &DbPool, user_id: i64, req: &NewBundle) -> Result<Bundle, AppError> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("名称不能为空".into()));
    }
    let search_json = serde_json::to_string(&req.search).map_err(AppError::internal)?;
    let now = Utc::now().to_rfc3339();
    let next_order: i64 = crate::db::q_scalar::<()>(
        "SELECT COALESCE(MAX(sort_order), 0) + 1 FROM bundles WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?
    .get_i64(0);
    let insert = crate::db::q(
        "INSERT INTO bundles (user_id, name, search_json, created_at, sort_order) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(name)
    .bind(&search_json)
    .bind(&now)
    .bind(next_order);
    let id = match pool {
        DbPool::Postgres(_) => insert.fetch_one(pool).await?.get_i64(0),
        // SQLite 与 MySQL 同形态：INSERT 响应自带自增回读（MySQL 为 last_insert_id）。
        DbPool::Sqlite(_) | DbPool::MySql(_) => insert.execute(pool).await?.last_insert_rowid(),
    };
    get(pool, user_id, id)
        .await?
        .ok_or_else(|| AppError::internal("创建后读取 bundle 失败"))
}

/// 按 id 获取（限本人）。
pub async fn get(pool: &DbPool, user_id: i64, id: i64) -> Result<Option<Bundle>, AppError> {
    let row = crate::db::q_as::<(i64, i64, String, String, String, i64)>(
        "SELECT id, user_id, name, search_json, created_at, sort_order FROM bundles WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| Bundle {
        id: r.get_i64(0),
        user_id: r.get_i64(1),
        name: r.get_string(2),
        search_json: r.get_string(3),
        created_at: r.get_string(4),
        sort_order: r.get_i64(5),
    }))
}

/// 列出全部 bundles。
pub async fn list(pool: &DbPool, user_id: i64) -> Result<Vec<Bundle>, AppError> {
    let rows = crate::db::q_as::<(i64, i64, String, String, String, i64)>(
        "SELECT id, user_id, name, search_json, created_at, sort_order FROM bundles WHERE user_id = ? ORDER BY sort_order ASC, created_at ASC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Bundle {
            id: r.get_i64(0),
            user_id: r.get_i64(1),
            name: r.get_string(2),
            search_json: r.get_string(3),
            created_at: r.get_string(4),
            sort_order: r.get_i64(5),
        })
        .collect())
}

/// 重新排序 bundles（对齐 linkding 拖拽排序）：按传入 id 顺序重写 sort_order。
pub async fn reorder(pool: &DbPool, user_id: i64, ordered_ids: &[i64]) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    for (idx, id) in ordered_ids.iter().enumerate() {
        crate::db::q("UPDATE bundles SET sort_order = ? WHERE id = ? AND user_id = ?")
            .bind((idx + 1) as i64)
            .bind(*id)
            .bind(user_id)
            .execute_tx(&mut tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 更新名称/过滤条件。
pub async fn update(
    pool: &DbPool,
    user_id: i64,
    id: i64,
    name: Option<String>,
    search: Option<BundleSearch>,
) -> Result<Option<Bundle>, AppError> {
    let existing = get(pool, user_id, id).await?.ok_or(AppError::NotFound)?;
    let new_name = name.unwrap_or(existing.name);
    if new_name.trim().is_empty() {
        return Err(AppError::BadRequest("名称不能为空".into()));
    }
    let new_json = match search {
        Some(s) => serde_json::to_string(&s).map_err(AppError::internal)?,
        None => existing.search_json,
    };
    crate::db::q("UPDATE bundles SET name = ?, search_json = ? WHERE id = ? AND user_id = ?")
        .bind(new_name.trim())
        .bind(&new_json)
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    get(pool, user_id, id).await
}

/// 删除 bundle（不影响书签）。
pub async fn delete(pool: &DbPool, user_id: i64, id: i64) -> Result<bool, AppError> {
    let result = crate::db::q("DELETE FROM bundles WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 把 bundle 的过滤条件应用到 ListParams（仅覆盖 bundle 中显式保存的字段）。
pub fn apply_to_params(bundle: &Bundle, params: &mut ListParams) -> Result<(), AppError> {
    let search: BundleSearch = serde_json::from_str(&bundle.search_json)
        .map_err(|e| AppError::internal(format!("过滤条件解析失败: {e}")))?;
    if let Some(f) = search.filter {
        // 兼容旧字段：filter='unread'/'read' 映射为阅读状态过滤。
        match f.as_str() {
            "unread" => {
                if search.unread.is_none() {
                    params.unread = Some(true);
                }
                params.filter = Some("active".into());
            }
            "read" => {
                if search.unread.is_none() {
                    params.unread = Some(false);
                }
                params.filter = Some("active".into());
            }
            _ => params.filter = Some(f),
        }
    }
    if search.unread.is_some() {
        params.unread = search.unread;
    }
    if !search.tags_and.is_empty() {
        params.tags_and = search.tags_and;
    }
    if !search.tags_or.is_empty() {
        params.tags_or = search.tags_or;
    }
    if !search.tags_not.is_empty() {
        params.tags_not = search.tags_not;
    }
    if search.shared.is_some() {
        params.shared = search.shared;
    }
    if let Some(q) = search.q {
        params.q = Some(q);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bundle_crud_roundtrip() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        // 创建。
        let req = NewBundle {
            name: "GitHub".into(),
            search: BundleSearch {
                filter: Some("all".into()),
                unread: None,
                tags_and: vec!["git".into()],
                tags_or: vec![],
                tags_not: vec![],
                shared: None,
                q: Some("github".into()),
            },
        };
        let b = create(&pool, u.id, &req).await.unwrap();
        assert_eq!(b.name, "GitHub");
        // 读取。
        let loaded = get(&pool, u.id, b.id).await.unwrap().unwrap();
        assert_eq!(loaded.search_json, b.search_json);
        // 列表。
        assert_eq!(list(&pool, u.id).await.unwrap().len(), 1);
        // 更新。
        let updated = update(&pool, u.id, b.id, Some("Renamed".into()), None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.name, "Renamed");
        // 删除。
        assert!(delete(&pool, u.id, b.id).await.unwrap());
        assert!(get(&pool, u.id, b.id).await.unwrap().is_none());
    }

    #[test]
    fn apply_to_params_merges() {
        let bundle = Bundle {
            id: 1,
            user_id: 1,
            name: "x".into(),
            search_json: r#"{"filter":"archived","tags_and":["a"],"q":"rust"}"#.into(),
            created_at: String::new(),
            sort_order: 0,
        };
        let mut params = ListParams::default();
        apply_to_params(&bundle, &mut params).unwrap();
        assert_eq!(params.filter.as_deref(), Some("archived"));
        assert_eq!(params.tags_and, vec!["a"]);
        assert_eq!(params.q.as_deref(), Some("rust"));
    }

    #[test]
    fn apply_rejects_bad_json() {
        let bundle = Bundle {
            id: 1,
            user_id: 1,
            name: "x".into(),
            search_json: "not-json".into(),
            created_at: String::new(),
            sort_order: 0,
        };
        let mut params = ListParams::default();
        assert!(apply_to_params(&bundle, &mut params).is_err());
    }
    /// TODO9 回归：独立 unread 字段生效；旧 filter='unread'/'read' 兼容映射。
    #[test]
    fn apply_to_params_unread_field_and_legacy_filter() {
        // 新字段：unread=true/false 直接生效。
        let bundle = Bundle {
            id: 1,
            user_id: 1,
            name: "x".into(),
            search_json: r#"{"unread":true}"#.into(),
            created_at: String::new(),
            sort_order: 0,
        };
        let mut params = ListParams::default();
        apply_to_params(&bundle, &mut params).unwrap();
        assert_eq!(params.unread, Some(true));

        // 旧字段：filter='unread' → unread=true 且 filter 归一为 active。
        let legacy = Bundle {
            id: 2,
            user_id: 1,
            name: "y".into(),
            search_json: r#"{"filter":"unread"}"#.into(),
            created_at: String::new(),
            sort_order: 0,
        };
        let mut params = ListParams::default();
        apply_to_params(&legacy, &mut params).unwrap();
        assert_eq!(params.unread, Some(true));
        assert_eq!(params.filter.as_deref(), Some("active"));

        // 旧字段：filter='read' → unread=false。
        let legacy_read = Bundle {
            id: 3,
            user_id: 1,
            name: "z".into(),
            search_json: r#"{"filter":"read"}"#.into(),
            created_at: String::new(),
            sort_order: 0,
        };
        let mut params = ListParams::default();
        apply_to_params(&legacy_read, &mut params).unwrap();
        assert_eq!(params.unread, Some(false));

        // 新字段优先于旧字段：两者同时存在时以 unread 为准。
        let both = Bundle {
            id: 4,
            user_id: 1,
            name: "w".into(),
            search_json: r#"{"filter":"read","unread":true}"#.into(),
            created_at: String::new(),
            sort_order: 0,
        };
        let mut params = ListParams::default();
        apply_to_params(&both, &mut params).unwrap();
        assert_eq!(params.unread, Some(true));
    }
}
