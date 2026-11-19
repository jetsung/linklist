use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};

#[derive(Deserialize, Default)]
pub struct AuditQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    /// 按动作过滤（如 password_changed / token_revoked）。
    pub action: Option<String>,
    /// 按用户名过滤（关联 users 表）。
    pub username: Option<String>,
}

/// 审计日志筛选条件：action / username 任一可空。
struct AuditFilter {
    action: Option<String>,
    username: Option<String>,
}

impl AuditFilter {
    /// 动态拼 WHERE 子句与绑定参数（?N 顺序与绑定一致）；prefix 为表别名（DELETE 单表时传空串）。
    fn clause_and_binds(&self, prefix: &str) -> (String, Vec<Option<String>>) {
        let col = |name: &str| {
            // username 在 users 表（别名 u），其余在 audit_logs 表（别名 a）。
            if name == "username" {
                "u.username".to_string()
            } else if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}.{name}")
            }
        };
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<Option<String>> = Vec::new();
        if self.action.is_some() {
            binds.push(self.action.clone());
            clauses.push(format!("{} = ?{}", col("action"), binds.len()));
        }
        if self.username.is_some() {
            binds.push(self.username.clone());
            clauses.push(format!("{} = ?{}", col("username"), binds.len()));
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        (where_sql, binds)
    }

    /// DELETE 单表变体：username 用子查询解析 user_id（无 users JOIN）。
    fn clause_and_binds_subquery(&self) -> (String, Vec<Option<String>>) {
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<Option<String>> = Vec::new();
        if self.action.is_some() {
            binds.push(self.action.clone());
            clauses.push(format!("action = ?{}", binds.len()));
        }
        if self.username.is_some() {
            binds.push(self.username.clone());
            clauses.push(format!(
                "user_id IN (SELECT id FROM users WHERE username = ?{})",
                binds.len()
            ));
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        (where_sql, binds)
    }
}

/// `GET /api/v1/admin/audit-logs`：审计日志查询（管理员）。
pub async fn list_audit_logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !auth.0.is_admin {
        return Err(AppError::Forbidden);
    }
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 200);

    let filter = AuditFilter {
        action: params.action.clone(),
        username: params.username.clone(),
    };
    let (where_sql, binds) = filter.clause_and_binds("a");

    let total_sql = format!(
        "SELECT COUNT(*) FROM audit_logs a LEFT JOIN users u ON u.id = a.user_id {where_sql}"
    );
    let mut total_query = crate::db::q_scalar::<i64>(&total_sql);
    for b in &binds {
        total_query = total_query.bind(b.clone());
    }
    let total: i64 = total_query.fetch_one(&state.pool).await?.get_i64(0);

    let list_sql = format!(
        "SELECT a.id, a.user_id, u.username, a.action, a.target, a.created_at
         FROM audit_logs a LEFT JOIN users u ON u.id = a.user_id
         {where_sql}
         ORDER BY a.id DESC LIMIT {} OFFSET {}",
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
                "user_id": r.get_named_opt_i64("user_id"),
                "username": r.get_named_opt_string("username"),
                "action": r.get_named_string("action"),
                "target": r.get_named_string("target"),
                "created_at": r.get_named_string("created_at"),
            })
        })
        .collect();
    Ok(Json(
        serde_json::json!({"results": items, "total": total, "page": page, "per_page": per_page}),
    ))
}

/// `DELETE /api/v1/admin/audit-logs`：清空审计日志（支持与列表相同的 action/username 筛选）。
pub async fn clear_audit_logs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<AuditQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    if !auth.0.is_admin {
        return Err(AppError::Forbidden);
    }
    let filter = AuditFilter {
        action: params.action.clone(),
        username: params.username.clone(),
    };
    // DELETE 单表无 users 别名：按用户名过滤时用子查询匹配 user_id。
    let (where_sql, binds) = if params.username.is_some() {
        filter.clause_and_binds_subquery()
    } else {
        filter.clause_and_binds("")
    };
    let sql = format!("DELETE FROM audit_logs {where_sql}");
    let mut query = crate::db::q(&sql);
    for b in &binds {
        query = query.bind(b.clone());
    }
    let result = query.execute(&state.pool).await?;
    Ok(Json(serde_json::json!({"deleted": result.rows_affected()})))
}
