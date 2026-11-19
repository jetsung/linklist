use crate::db::DbPool;
use serde::Deserialize;
use serde_json::json;

use crate::bookmarks::crud;
use crate::bookmarks::tags;
use crate::error::AppError;

/// 逐项结果。
#[derive(Debug, serde::Serialize)]
pub struct ItemResult {
    pub id: i64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 汇总结果。
#[derive(Debug, serde::Serialize)]
pub struct BulkResult {
    pub results: Vec<ItemResult>,
    pub succeeded: usize,
    pub failed: usize,
}

impl BulkResult {
    fn push(&mut self, id: i64, r: Result<(), AppError>) {
        match r {
            Ok(()) => {
                self.results.push(ItemResult {
                    id,
                    ok: true,
                    error: None,
                });
                self.succeeded += 1;
            }
            Err(e) => {
                self.results.push(ItemResult {
                    id,
                    ok: false,
                    error: Some(e.message()),
                });
                self.failed += 1;
            }
        }
    }
}

/// 批量加标签。
pub async fn add_tags(
    pool: &DbPool,
    user_id: i64,
    ids: &[i64],
    tag_names: &[String],
) -> Result<BulkResult, AppError> {
    let mut out = BulkResult {
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
    };
    for &id in ids {
        let r = (|| async {
            let existing = crud::get(pool, user_id, id)
                .await?
                .ok_or(AppError::NotFound)?;
            let mut current = tags::tags_for_bookmark(pool, id).await?;
            for t in tag_names {
                if !current.iter().any(|c| c.eq_ignore_ascii_case(t)) {
                    current.push(t.clone());
                }
            }
            tags::set_bookmark_tags(pool, user_id, existing.id, &current).await
        })()
        .await;
        out.push(id, r);
    }
    Ok(out)
}

/// 批量删标签。
pub async fn remove_tags(
    pool: &DbPool,
    user_id: i64,
    ids: &[i64],
    tag_names: &[String],
) -> Result<BulkResult, AppError> {
    let mut out = BulkResult {
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
    };
    for &id in ids {
        let r = (|| async {
            let existing = crud::get(pool, user_id, id)
                .await?
                .ok_or(AppError::NotFound)?;
            let current = tags::tags_for_bookmark(pool, id).await?;
            let kept: Vec<String> = current
                .into_iter()
                .filter(|c| !tag_names.iter().any(|t| c.eq_ignore_ascii_case(t)))
                .collect();
            tags::set_bookmark_tags(pool, user_id, existing.id, &kept).await
        })()
        .await;
        out.push(id, r);
    }
    Ok(out)
}

/// 批量标记已读/未读。
pub async fn mark_read(
    pool: &DbPool,
    user_id: i64,
    ids: &[i64],
    unread: bool,
) -> Result<BulkResult, AppError> {
    let mut out = BulkResult {
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
    };
    for &id in ids {
        let r = crud::update(
            pool,
            user_id,
            id,
            &crud::UpdateBookmark {
                is_unread: Some(unread),
                ..Default::default()
            },
        )
        .await
        .map(|_| ());
        out.push(id, r);
    }
    Ok(out)
}

/// 批量设置共享状态（Share / Unshare）。
pub async fn set_shared(
    pool: &DbPool,
    user_id: i64,
    ids: &[i64],
    shared: bool,
) -> Result<BulkResult, AppError> {
    let mut out = BulkResult {
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
    };
    for &id in ids {
        let r = crud::update(
            pool,
            user_id,
            id,
            &crud::UpdateBookmark {
                is_shared: Some(shared),
                ..Default::default()
            },
        )
        .await
        .map(|_| ());
        out.push(id, r);
    }
    Ok(out)
}

/// 批量删除。
pub async fn delete_many(pool: &DbPool, user_id: i64, ids: &[i64]) -> Result<BulkResult, AppError> {
    let mut out = BulkResult {
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
    };
    for &id in ids {
        let r = crud::delete(pool, user_id, id)
            .await
            .map(|ok| if ok { Ok(()) } else { Err(AppError::NotFound) });
        out.push(id, r.and_then(|inner| inner));
    }
    Ok(out)
}

/// 批量请求体。
#[derive(Debug, Deserialize)]
pub struct BulkIdsRequest {
    pub ids: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct BulkTagsRequest {
    pub ids: Vec<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// 统一 JSON 响应。
pub fn to_json(r: &BulkResult) -> serde_json::Value {
    json!({
        "results": r.results,
        "succeeded": r.succeeded,
        "failed": r.failed,
    })
}
/// 批量归档/取消归档。
pub async fn set_archived(
    pool: &DbPool,
    user_id: i64,
    ids: &[i64],
    archived: bool,
) -> Result<BulkResult, AppError> {
    let mut out = BulkResult {
        results: Vec::new(),
        succeeded: 0,
        failed: 0,
    };
    for &id in ids {
        let r = crud::update(
            pool,
            user_id,
            id,
            &crud::UpdateBookmark {
                is_archived: Some(archived),
                ..Default::default()
            },
        )
        .await
        .map(|_| ());
        out.push(id, r);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup() -> (DbPool, i64, Vec<i64>) {
        let pool = db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for url in ["https://a.com", "https://b.com", "https://c.com"] {
            let b = crud::create(
                &pool,
                u.id,
                &crud::NewBookmark {
                    url: url.into(),
                    title: Some(String::new()),
                    description: Some(String::new()),
                    notes: Some(String::new()),
                    is_unread: true,
                    is_shared: false,
                    tags: vec!["old".into()],
                },
            )
            .await
            .unwrap();
            ids.push(b.id);
        }
        (pool, u.id, ids)
    }

    #[tokio::test]
    async fn bulk_add_and_remove_tags() {
        let (pool, uid, ids) = setup().await;
        let r = add_tags(&pool, uid, &ids, &["new".into()]).await.unwrap();
        assert_eq!(r.succeeded, 3);
        for &id in &ids {
            let t = tags::tags_for_bookmark(&pool, id).await.unwrap();
            assert!(t.contains(&"old".into()) && t.contains(&"new".into()));
        }
        let r = remove_tags(&pool, uid, &ids, &["old".into()])
            .await
            .unwrap();
        assert_eq!(r.succeeded, 3);
        let t = tags::tags_for_bookmark(&pool, ids[0]).await.unwrap();
        assert_eq!(t, vec!["new"]);
    }

    #[tokio::test]
    async fn bulk_mark_read_partial_failure() {
        let (pool, uid, ids) = setup().await;
        // 混入不存在的 id。
        let mut mixed = ids.clone();
        mixed.push(9999);
        let r = mark_read(&pool, uid, &mixed, false).await.unwrap();
        assert_eq!(r.succeeded, 3);
        assert_eq!(r.failed, 1);
        assert!(!r.results[3].ok);
        for &id in &ids {
            let b = crud::get(&pool, uid, id).await.unwrap().unwrap();
            assert!(!b.is_unread);
        }
    }

    #[tokio::test]
    async fn bulk_delete_partial_failure() {
        let (pool, uid, ids) = setup().await;
        let mut mixed = ids.clone();
        mixed.push(9999);
        let r = delete_many(&pool, uid, &mixed).await.unwrap();
        assert_eq!(r.succeeded, 3);
        assert_eq!(r.failed, 1);
        assert_eq!(r.results[3].error.as_deref(), Some("资源不存在"));
        let (remaining, _) = crud::list(&pool, uid, &Default::default()).await.unwrap();
        assert_eq!(remaining.len(), 0);
    }
}
