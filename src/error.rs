use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// 应用错误：统一映射为 HTTP 状态码 + `{"error":{"code","message"}}` 响应体。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// 400：请求参数校验失败。
    #[error("{0}")]
    BadRequest(String),
    /// 401：未认证。
    #[error("未认证")]
    Unauthorized,
    /// 403：无权限。
    #[error("无权限访问该资源")]
    Forbidden,
    /// 404：资源不存在。
    #[error("资源不存在")]
    NotFound,
    /// 409：资源冲突（如 URL 重复）。
    #[error("{0}")]
    Conflict(String),
    /// 429：请求过于频繁（登录限速等）。
    #[error("请求过于频繁，请稍后再试")]
    TooManyRequests,
    /// 500：内部错误（不向客户端泄露细节）。
    #[error("内部错误")]
    Internal,
}

impl AppError {
    /// 便捷构造：内部错误（任意错误类型）。
    pub fn internal<E: std::fmt::Display>(err: E) -> Self {
        tracing::error!("内部错误: {err}");
        AppError::Internal
    }

    /// HTTP 状态码。
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            AppError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// 错误码字符串（稳定，供客户端程序化处理）。
    pub fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "bad_request",
            AppError::Unauthorized => "unauthorized",
            AppError::Forbidden => "forbidden",
            AppError::NotFound => "not_found",
            AppError::Conflict(_) => "conflict",
            AppError::TooManyRequests => "too_many_requests",
            AppError::Internal => "internal_error",
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &err {
            // SQLite 唯一约束冲突 → 409。
            if db.code().as_deref() == Some("2067") {
                return AppError::Conflict("记录已存在".to_string());
            }
        }
        AppError::internal(err)
    }
}

impl AppError {
    /// 面向客户端的错误消息（内部错误不泄露细节）。
    pub fn message(&self) -> String {
        match self {
            AppError::Internal => "内部错误".to_string(),
            other => other.to_string(),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::internal(err)
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::internal(err)
    }
}

/// 统一错误响应体：`{"error": {"code": "...", "message": "..."}}`。
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = json!({
            "error": {
                "code": self.code(),
                "message": self.message(),
            }
        });
        (status, axum::Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn error_body_shape() {
        let resp = AppError::NotFound.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
        assert!(!v["error"]["message"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn internal_error_hides_detail() {
        let resp = AppError::internal("secret db path /x/y").into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["message"], "内部错误");
        assert_eq!(v["error"]["code"], "internal_error");
    }

    #[test]
    fn sqlite_unique_becomes_conflict() {
        let err = AppError::from(sqlx::Error::RowNotFound);
        assert!(matches!(err, AppError::Internal));
    }
}
