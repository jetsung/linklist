use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};

/// 管理员守卫：非管理员 403。
fn require_admin(auth: &AuthUser) -> Result<(), AppError> {
    if auth.0.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub is_admin: bool,
}

/// `POST /api/v1/admin/users`：创建用户。
pub async fn create_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<CreateUserRequest>,
) -> Result<(axum::http::StatusCode, Json<serde_json::Value>), AppError> {
    require_admin(&auth)?;
    let username = req.username.trim().to_string();
    if username.is_empty() {
        return Err(AppError::BadRequest("用户名不能为空".into()));
    }
    if req.password.len() < 8 {
        return Err(AppError::BadRequest("口令至少 8 位".into()));
    }
    match crate::models::create_user(&state.pool, &username, &req.password, req.is_admin).await {
        Ok(u) => Ok((
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({"id": u.id, "username": u.username, "is_admin": u.is_admin})),
        )),
        Err(crate::error::AppError::Conflict(msg)) => Err(AppError::Conflict(msg)),
        Err(e) => Err(e),
    }
}

/// `GET /api/v1/admin/users`：用户列表。
pub async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    let rows = crate::db::q(
        "SELECT id, username, is_admin, is_active, date_joined FROM users ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get_named_i64("id"),
                "username": r.get_named_string("username"),
                "is_admin": r.get_named_i64("is_admin") != 0,
                "is_active": r.get_named_i64("is_active") != 0,
                "date_joined": r.get_named_string("date_joined"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"results": items})))
}

/// `POST /api/v1/admin/users/{id}/deactivate`：停用用户（会话/Token 立即失效——认证时校验 is_active）。
pub async fn deactivate_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    if id == auth.0.id {
        return Err(AppError::BadRequest("不能停用自己".into()));
    }
    let result = crate::db::q("UPDATE users SET is_active = 0 WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    // 吊销该用户全部 API Token。
    crate::db::q("UPDATE api_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id)
        .execute(&state.pool)
        .await?;
    crate::models::audit(
        &state.pool,
        Some(id),
        crate::models::audit_action::USER_DEACTIVATED,
        "admin",
    )
    .await;
    Ok(Json(serde_json::json!({"ok": true})))
}

/// `POST /api/v1/admin/users/{id}/activate`：启用用户。
pub async fn activate_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    let result = crate::db::q("UPDATE users SET is_active = 1 WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

/// `POST /api/v1/admin/users/{id}/reset-password`：管理员重置口令。
pub async fn reset_password(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    if req.password.len() < 8 {
        return Err(AppError::BadRequest("口令至少 8 位".into()));
    }
    let result = crate::models::update_password(&state.pool, id, &req.password).await;
    match result {
        Ok(()) => {
            crate::models::audit(
                &state.pool,
                Some(id),
                crate::models::audit_action::PASSWORD_RESET_BY_ADMIN,
                "admin",
            )
            .await;
            Ok(Json(serde_json::json!({"ok": true})))
        }
        Err(_) => Err(AppError::NotFound),
    }
}

#[derive(Deserialize)]
pub struct ResetPasswordRequest {
    pub password: String,
}

#[derive(Deserialize, Default)]
pub struct AdminListQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// `GET /api/v1/admin/data/bookmarks`：原始数据分页查询。
pub async fn list_all_bookmarks(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AdminListQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 200);
    let total: i64 = crate::db::q_scalar::<()>("SELECT COUNT(*) FROM bookmarks")
        .fetch_one(&state.pool)
        .await?
        .get_i64(0);
    let rows = crate::db::q(
        "SELECT id, user_id, url, title, is_archived, is_shared, created_at FROM bookmarks ORDER BY id LIMIT ? OFFSET ?",
    )
    .bind(per_page)
    .bind((page - 1) * per_page)
    .fetch_all(&state.pool)
    .await?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get_named_i64("id"),
                "user_id": r.get_named_i64("user_id"),
                "url": r.get_named_string("url"),
                "title": r.get_named_string("title"),
                "is_archived": r.get_named_i64("is_archived") != 0,
                "is_shared": r.get_named_i64("is_shared") != 0,
                "created_at": r.get_named_string("created_at"),
            })
        })
        .collect();
    Ok(Json(
        serde_json::json!({"results": items, "total": total, "page": page, "per_page": per_page}),
    ))
}

/// 抓取任务查询参数：分页 + kind/status 筛选。
#[derive(Deserialize, Default)]
pub struct AdminTaskQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    /// 按任务类型过滤（metadata / snapshot / archive_lookup / preview）。
    pub kind: Option<String>,
    /// 按状态过滤（pending / running / success / failed）。
    pub status: Option<String>,
}

/// 抓取任务筛选条件：kind / status 任一可空。
#[derive(Default)]
struct TaskFilter {
    kind: Option<String>,
    status: Option<String>,
}

impl TaskFilter {
    /// 动态拼 WHERE 子句与绑定参数；prefix 为表别名（DELETE 单表时传空串）。
    fn clause_and_binds(&self, prefix: &str) -> (String, Vec<String>) {
        let col = |name: &str| {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}.{name}")
            }
        };
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<String> = Vec::new();
        if let Some(k) = self.kind.as_deref().filter(|s| !s.trim().is_empty()) {
            binds.push(k.to_string());
            clauses.push(format!("{} = ?{}", col("kind"), binds.len()));
        }
        if let Some(s) = self.status.as_deref().filter(|s| !s.trim().is_empty()) {
            binds.push(s.to_string());
            clauses.push(format!("{} = ?{}", col("status"), binds.len()));
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        (where_sql, binds)
    }
}

/// `GET /api/v1/admin/tasks`：抓取任务记录查询（支持 kind/status 筛选）。
pub async fn list_tasks(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AdminTaskQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 200);
    let filter = TaskFilter {
        kind: params.kind.clone(),
        status: params.status.clone(),
    };
    let (where_sql, binds) = filter.clause_and_binds("t");
    let total_sql = format!("SELECT COUNT(*) FROM capture_tasks t {where_sql}");
    let mut total_query = crate::db::q_scalar::<i64>(&total_sql);
    for b in &binds {
        total_query = total_query.bind(b.clone());
    }
    let total: i64 = total_query.fetch_one(&state.pool).await?.get_i64(0);
    let list_sql = format!(
        "SELECT t.id, t.bookmark_id, t.kind, t.status, t.attempts, t.last_error, t.created_at,
                b.url AS bookmark_url, b.title AS bookmark_title
         FROM capture_tasks t LEFT JOIN bookmarks b ON b.id = t.bookmark_id
         {where_sql} ORDER BY t.id DESC LIMIT {} OFFSET {}",
        per_page,
        (page - 1) * per_page
    );
    let mut list_query = crate::db::q(&list_sql);
    for b in &binds {
        list_query = list_query.bind(b.clone());
    }
    let rows = list_query.fetch_all(&state.pool).await?;
    let items: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get_named_i64("id"),
                "bookmark_id": r.get_named_i64("bookmark_id"),
                "kind": r.get_named_string("kind"),
                "status": r.get_named_string("status"),
                "attempts": r.get_u32_named("attempts"),
                "last_error": r.get_named_opt_string("last_error"),
                "created_at": r.get_named_string("created_at"),
                "bookmark_url": r.get_named_opt_string("bookmark_url"),
                "bookmark_title": r.get_named_opt_string("bookmark_title"),
            })
        })
        .collect();
    Ok(Json(
        serde_json::json!({"results": items, "total": total, "page": page, "per_page": per_page}),
    ))
}

/// `DELETE /api/v1/admin/tasks`：清空抓取任务记录（支持与列表相同的 kind/status 筛选）。
pub async fn clear_tasks(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AdminTaskQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    let filter = TaskFilter {
        kind: params.kind.clone(),
        status: params.status.clone(),
    };
    let (where_sql, binds) = filter.clause_and_binds("");
    let sql = format!("DELETE FROM capture_tasks {where_sql}");
    let mut query = crate::db::q(&sql);
    for b in &binds {
        query = query.bind(b.clone());
    }
    let result = query.execute(&state.pool).await?;
    Ok(Json(serde_json::json!({"deleted": result.rows_affected()})))
}

/// `GET /api/v1/admin/settings`：读取全局设置。
pub async fn get_settings(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    let rows = crate::db::q("SELECT key, value FROM settings")
        .fetch_all(&state.pool)
        .await?;
    let mut map = serde_json::Map::new();
    for r in &rows {
        let k: String = r.get_named_string("key");
        let v: String = r.get_named_string("value");
        map.insert(k, serde_json::Value::String(v));
    }
    // 显示生效默认值（settings 表未写入时也返回）。
    for (k, v) in [
        ("landing_page", "login"),
        ("guest_profile_user", ""),
        ("enable_link_prefetch", "false"),
    ] {
        map.entry(k.to_string())
            .or_insert_with(|| serde_json::Value::String(v.to_string()));
    }
    Ok(Json(serde_json::Value::Object(map)))
}

/// 允许的全局设置键与取值校验。
fn validate_setting(key: &str, value: &str) -> Result<(), AppError> {
    match key {
        "guest_sharing" => {
            if !["true", "false", "1", "0"].contains(&value) {
                return Err(AppError::BadRequest(
                    "guest_sharing 仅允许 true/false".into(),
                ));
            }
            Ok(())
        }
        "max_upload_mb" => {
            let n: i64 = value
                .parse()
                .map_err(|_| AppError::BadRequest("max_upload_mb 须为整数".into()))?;
            if !(1..=1024).contains(&n) {
                return Err(AppError::BadRequest("max_upload_mb 须在 1-1024".into()));
            }
            Ok(())
        }
        "landing_page" => {
            if !["login", "shared_bookmarks"].contains(&value) {
                return Err(AppError::BadRequest(
                    "landing_page 仅允许 login/shared_bookmarks".into(),
                ));
            }
            Ok(())
        }
        "guest_profile_user" => {
            // 空串表示使用标准访客资料；否则必须是数字用户 ID（存在性在 set_setting 中校验）。
            if value.is_empty() {
                return Ok(());
            }
            value
                .parse::<i64>()
                .map_err(|_| AppError::BadRequest("guest_profile_user 须为用户 ID".into()))?;
            Ok(())
        }
        "enable_link_prefetch" => {
            if !["true", "false"].contains(&value) {
                return Err(AppError::BadRequest(
                    "enable_link_prefetch 仅允许 true/false".into(),
                ));
            }
            Ok(())
        }
        _ => Err(AppError::BadRequest(format!("未知设置键: {key}"))),
    }
}
/// `PUT /api/v1/admin/settings`：修改全局设置（白名单 + 值校验）。
pub async fn set_setting(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<serde_json::Map<String, serde_json::Value>>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&auth)?;
    for (k, v) in &req {
        let val = v
            .as_str()
            .ok_or_else(|| AppError::BadRequest(format!("设置 {k} 的值须为字符串")))?;
        validate_setting(k, val)?;
        if k == "guest_profile_user" && !val.is_empty() {
            let id: i64 = val.parse().unwrap_or(0);
            let exists = crate::db::q_scalar::<i64>("SELECT COUNT(*) FROM users WHERE id = ?")
                .bind(id)
                .fetch_one(&state.pool)
                .await?
                .get_i64(0);
            if exists == 0 {
                return Err(AppError::BadRequest("guest_profile_user 用户不存在".into()));
            }
        }
        crate::db::q(crate::db::sql_settings_upsert(state.pool.dialect()))
            .bind(k)
            .bind(val)
            .execute(&state.pool)
            .await?;
        crate::models::audit(
            &state.pool,
            Some(auth.0.id),
            crate::models::audit_action::SETTINGS_CHANGED,
            k,
        )
        .await;
    }
    get_settings(State(state), auth).await
}
