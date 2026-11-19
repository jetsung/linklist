use crate::db::{DbPool, DbRow};
use chrono::Utc;
use serde::Serialize;

use crate::auth::password;
use crate::error::AppError;

/// 用户（含 profile 聚合，登录后使用）。
#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub is_active: bool,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub date_joined: String,
}

/// 审计日志动作常量。
pub mod audit_action {
    pub const LOGIN: &str = "login";
    pub const LOGIN_FAILED: &str = "login_failed";
    pub const LOGOUT: &str = "logout";
    pub const PASSWORD_CHANGED: &str = "password_changed";
    pub const TOKEN_CREATED: &str = "token_created";
    pub const TOKEN_REVOKED: &str = "token_revoked";
    pub const SETUP: &str = "setup";
    pub const USER_DEACTIVATED: &str = "user_deactivated";
    /// 预留：管理员重新启用用户时使用。
    #[allow(dead_code)]
    pub const USER_ACTIVATED: &str = "user_activated";
    pub const PASSWORD_RESET_BY_ADMIN: &str = "password_reset_by_admin";
    pub const SETTINGS_CHANGED: &str = "settings_changed";
}

/// 写入审计日志。
pub async fn audit(pool: &DbPool, user_id: Option<i64>, action: &str, target: &str) {
    let now = Utc::now().to_rfc3339();
    let _ = crate::db::q(
        "INSERT INTO audit_logs (user_id, action, target, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(action)
    .bind(target)
    .bind(now)
    .execute(pool)
    .await;
}

/// 按用户名查用户。
pub async fn find_by_username(pool: &DbPool, username: &str) -> Result<Option<User>, AppError> {
    user_row_by(pool, "username", username.to_string()).await
}

/// 按 id 查用户。
pub async fn find_by_id(pool: &DbPool, id: i64) -> Result<Option<User>, AppError> {
    user_row_by(pool, "id", id).await
}

/// 统一的按列查询用户（列名固定白名单，无注入面）。
async fn user_row_by<V: Into<crate::db::DbValue>>(
    pool: &DbPool,
    col: &str,
    val: V,
) -> Result<Option<User>, AppError> {
    debug_assert!(matches!(col, "id" | "username"));
    let row = crate::db::q(format!(
        "SELECT id, username, password_hash, is_admin, is_active, date_joined FROM users WHERE {col} = ?"
    ))
    .bind(val)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(user_from_row))
}

/// 创建用户（含默认 profile）。
pub async fn create_user(
    pool: &DbPool,
    username: &str,
    plain_password: &str,
    is_admin: bool,
) -> Result<User, AppError> {
    let hash = password::hash_password(plain_password)?;
    let now = Utc::now().to_rfc3339();
    let user_id = match pool {
        DbPool::Postgres(_) => {
            let row = crate::db::q(
                "INSERT INTO users (username, password_hash, is_admin, is_active, date_joined) VALUES (?, ?, ?, TRUE, ?) RETURNING id",
            )
            .bind(username)
            .bind(&hash)
            .bind(is_admin)
            .bind(&now)
            .fetch_one(pool)
            .await?;
            row.get_i64(0)
        }
        // SQLite 与 MySQL 同形态：INSERT 响应自带自增回读（MySQL 为 last_insert_id）。
        DbPool::Sqlite(_) | DbPool::MySql(_) => {
            let result = crate::db::q(
                "INSERT INTO users (username, password_hash, is_admin, is_active, date_joined) VALUES (?, ?, ?, 1, ?)",
            )
            .bind(username)
            .bind(&hash)
            .bind(is_admin as i64)
            .bind(&now)
            .execute(pool)
            .await?;
            result.last_insert_rowid()
        }
    };
    crate::db::q("INSERT INTO user_profiles (user_id) VALUES (?)")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(User {
        id: user_id,
        username: username.to_string(),
        is_admin,
        is_active: true,
        password_hash: hash,
        date_joined: now,
    })
}

/// 更新用户口令。
pub async fn update_password(
    pool: &DbPool,
    user_id: i64,
    new_password: &str,
) -> Result<(), AppError> {
    let hash = password::hash_password(new_password)?;
    crate::db::q("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(&hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 按用户名取用户 id（不存在返回 None；CLI reset-password 用）。
pub async fn find_user_id_by_name(pool: &DbPool, username: &str) -> Result<Option<i64>, AppError> {
    crate::db::q("SELECT id FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?
        .map(|r| Ok(r.get_i64(0)))
        .transpose()
}

/// 预置管理员（`linklist init --admin-password` 与 Docker 首启 `LINKLIST_ADMIN_PASSWORD`
/// 共用的业务逻辑，对齐 Web 端 `POST /api/v1/auth/setup`）：仅当库中无任何用户时以
/// is_admin=true 创建 username 账号并记录 SETUP 审计；已有用户返回 None（跳过）。
pub async fn provision_admin(
    pool: &DbPool,
    username: &str,
    password: &str,
) -> Result<Option<User>, AppError> {
    if last_user_count(pool).await? > 0 {
        return Ok(None);
    }
    let user = create_user(pool, username, password, true).await?;
    audit(pool, Some(user.id), audit_action::SETUP, &user.username).await;
    Ok(Some(user))
}

/// 记录口令修改时间（settings 全局表，管理面板可见）。
pub async fn last_user_count(pool: &DbPool) -> Result<i64, AppError> {
    let n = crate::db::q_scalar::<()>("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?
        .get_i64(0);
    Ok(n)
}

/// 从数据库行组装用户（兼容 SQLite 整数布尔与 PostgreSQL 布尔）。
fn user_from_row(r: DbRow) -> User {
    User {
        id: r.get_named_i64("id"),
        username: r.get_named_string("username"),
        is_admin: r.get_named_i64("is_admin") != 0,
        is_active: r.get_named_i64("is_active") != 0,
        password_hash: r.get_named_string("password_hash"),
        date_joined: r.get_named_string("date_joined"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn create_and_find_user() {
        let pool = db::connect_in_memory().await.unwrap();
        let u = create_user(&pool, "alice", "pw123", true).await.unwrap();
        assert!(u.is_admin);
        let found = find_by_username(&pool, "alice").await.unwrap().unwrap();
        assert_eq!(found.id, u.id);
        assert!(password::verify_password("pw123", &found.password_hash).unwrap());
    }

    #[tokio::test]
    async fn duplicate_username_conflict() {
        let pool = db::connect_in_memory().await.unwrap();
        create_user(&pool, "bob", "pw", false).await.unwrap();
        let err = create_user(&pool, "bob", "pw2", false).await;
        assert!(matches!(err, Err(AppError::Conflict(_))));
    }

    #[tokio::test]
    async fn audit_log_written() {
        let pool = db::connect_in_memory().await.unwrap();
        let u = create_user(&pool, "alice", "pw12345", false).await.unwrap();
        audit(&pool, Some(u.id), audit_action::LOGIN, "web").await;
        let n: i64 =
            crate::db::q_scalar::<()>("SELECT COUNT(*) FROM audit_logs WHERE action='login'")
                .fetch_one(&pool)
                .await
                .unwrap()
                .get_i64(0);
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn audit_with_unknown_user_is_rejected_by_fk() {
        let pool = db::connect_in_memory().await.unwrap();
        // 外键约束下，不存在的 user_id 应导致插入失败（被 audit 静默吞掉）。
        audit(&pool, Some(999), audit_action::LOGIN, "web").await;
        let n: i64 = crate::db::q_scalar::<()>("SELECT COUNT(*) FROM audit_logs")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get_i64(0);
        assert_eq!(n, 0);
    }
}
