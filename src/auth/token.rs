use crate::db::DbPool;
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::models::{audit, audit_action};

/// 生成的 API Token（明文仅创建时返回一次）。
pub struct GeneratedToken {
    pub id: i64,
    pub plaintext: String,
}

/// 生成新 API Token：`lk_` + 32 字节随机数的 base64url；落库存 sha256。
pub async fn create_token(
    pool: &DbPool,
    user_id: i64,
    name: &str,
    ttl_days: u64,
) -> Result<GeneratedToken, AppError> {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes);
    let plaintext = format!(
        "lk_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    );
    let hash = hash_token(&plaintext);
    let now = chrono::Utc::now();
    let created = now.to_rfc3339();
    let expires_at = if ttl_days > 0 {
        Some((now + chrono::Duration::days(ttl_days as i64)).to_rfc3339())
    } else {
        None
    };
    let result = crate::db::q(
        "INSERT INTO api_tokens (user_id, name, token_hash, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(name)
    .bind(&hash)
    .bind(&created)
    .bind(expires_at.clone())
    .execute(pool)
    .await?;
    audit(pool, Some(user_id), audit_action::TOKEN_CREATED, name).await;
    Ok(GeneratedToken {
        id: result.last_insert_rowid(),
        plaintext,
    })
}

/// 生成 RSS 订阅 key：`fd_` + 24 字节随机数的 base64url。
pub fn generate_feed_key() -> String {
    let mut bytes = [0u8; 24];
    rand::fill(&mut bytes);
    format!(
        "fd_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    )
}

/// Token 明文 → sha256 hex（落库格式）。
pub fn hash_token(plaintext: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plaintext.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 按 hash 查找有效 Token（未吊销、未过期），返回 (token_id, user_id)。
pub async fn find_valid_token(
    pool: &DbPool,
    plaintext: &str,
) -> Result<Option<(i64, i64)>, AppError> {
    let hash = hash_token(plaintext);
    let row = crate::db::q_as::<(i64, i64, Option<String>)>(
        "SELECT id, user_id, expires_at FROM api_tokens WHERE token_hash = ? AND revoked_at IS NULL",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;
    match row {
        None => Ok(None),
        Some(r) => {
            let id = r.get_i64(0);
            let user_id = r.get_i64(1);
            let expires_at = r.get_opt_string(2);
            if let Some(exp) = expires_at {
                let exp_time = chrono::DateTime::parse_from_rfc3339(&exp)
                    .map_err(|e| AppError::internal(format!("过期时间解析失败: {e}")))?;
                if exp_time < chrono::Utc::now() {
                    return Ok(None);
                }
            }
            Ok(Some((id, user_id)))
        }
    }
}

/// 列出用户 Token（不含哈希）。
pub async fn list_tokens(pool: &DbPool, user_id: i64) -> Result<Vec<TokenInfo>, AppError> {
    let rows = crate::db::q_as::<(i64, String, String, Option<String>, Option<String>)>(
        "SELECT id, name, created_at, last_used_at, revoked_at FROM api_tokens WHERE user_id = ? AND revoked_at IS NULL ORDER BY id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| TokenInfo {
            id: r.get_i64(0),
            name: r.get_string(1),
            created_at: r.get_string(2),
            last_used_at: r.get_opt_string(3),
            revoked_at: r.get_opt_string(4),
        })
        .collect())
}

/// 吊销 Token（只能吊销自己的）。
pub async fn revoke_token(pool: &DbPool, user_id: i64, token_id: i64) -> Result<bool, AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let result = crate::db::q(
        "UPDATE api_tokens SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(&now)
    .bind(token_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    if result.rows_affected() > 0 {
        audit(
            pool,
            Some(user_id),
            audit_action::TOKEN_REVOKED,
            &token_id.to_string(),
        )
        .await;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// 更新最后使用时间。
pub async fn touch_last_used(pool: &DbPool, token_id: i64) {
    let now = chrono::Utc::now().to_rfc3339();
    let _ = crate::db::q("UPDATE api_tokens SET last_used_at = ? WHERE id = ?")
        .bind(&now)
        .bind(token_id)
        .execute(pool)
        .await;
}

/// Token 列表项。
#[derive(Debug, serde::Serialize)]
pub struct TokenInfo {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn create_and_validate_token() {
        let pool = db::connect_in_memory().await.unwrap();
        let user = crate::models::create_user(&pool, "u", "pw123456", false)
            .await
            .unwrap();
        let tok = create_token(&pool, user.id, "test", 0).await.unwrap();
        assert!(tok.plaintext.starts_with("lk_"));
        let found = find_valid_token(&pool, &tok.plaintext).await.unwrap();
        assert_eq!(found, Some((tok.id, user.id)));
        // 明文不等于哈希
        assert_ne!(tok.plaintext, hash_token(&tok.plaintext));
    }

    #[tokio::test]
    async fn revoked_token_invalid() {
        let pool = db::connect_in_memory().await.unwrap();
        let user = crate::models::create_user(&pool, "u", "pw123456", false)
            .await
            .unwrap();
        let tok = create_token(&pool, user.id, "t", 0).await.unwrap();
        assert!(revoke_token(&pool, user.id, tok.id).await.unwrap());
        assert_eq!(find_valid_token(&pool, &tok.plaintext).await.unwrap(), None);
    }

    #[tokio::test]
    async fn expired_token_invalid() {
        let pool = db::connect_in_memory().await.unwrap();
        let user = crate::models::create_user(&pool, "u", "pw123456", false)
            .await
            .unwrap();
        let tok = create_token(&pool, user.id, "t", 0).await.unwrap();
        // 手动把过期时间改为过去。
        crate::db::q("UPDATE api_tokens SET expires_at = '2000-01-01T00:00:00Z' WHERE id = ?")
            .bind(tok.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(find_valid_token(&pool, &tok.plaintext).await.unwrap(), None);
    }

    #[tokio::test]
    async fn wrong_token_invalid() {
        let pool = db::connect_in_memory().await.unwrap();
        assert_eq!(find_valid_token(&pool, "lk_bogus").await.unwrap(), None);
    }
}
