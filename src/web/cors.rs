//! CORS 中间件：对齐上游 `middlewares.py::CorsMiddleware`。
//!
//! - 仅作用于 `/api/` 路径；
//! - 未配置允许来源时完全关闭（不放行任何跨域请求）；
//! - 拦截 OPTIONS 预检并直接应答，避免预检请求被认证层拒绝；
//! - 不允许 credentials：跨域请求必须携带 API Token（Authorization 头）。

use axum::http::{HeaderValue, Method, Request, header};

/// 允许的 CORS 方法（对齐上游 Access-Control-Allow-Methods）。
#[allow(dead_code)] // 预留：CORS 预检响应后续消费
const ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

/// 预检结果缓存时长（秒）。
const PREFLIGHT_MAX_AGE: u32 = 86400;

/// 解析并规范化 origin；非法格式返回 None。
///
/// 对齐上游 `parse_origin`：仅接受 `scheme://host[:port]`（host 大小写不敏感，不含路径）。
pub fn parse_origin(origin: &str) -> Option<String> {
    let origin = origin.trim();
    if origin.is_empty() {
        return None;
    }
    let (scheme, rest) = origin.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(format!("{scheme}://{}", rest.to_ascii_lowercase()))
}

/// 判断请求是否为 CORS 预检。
fn is_preflight<B>(req: &Request<B>) -> bool {
    req.method() == Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

/// 为响应附加 CORS 头（origin 匹配时）。
fn cors_headers(
    headers: &mut axum::http::HeaderMap,
    origin: Option<&str>,
    allowed: &[String],
    preflight: bool,
) {
    // 对齐上游 patch_vary_headers：只要参与 CORS 就加 Vary: Origin
    //（含 origin 不在白名单的情况）。
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    let Some(origin) = origin else {
        return;
    };
    let Some(parsed) = parse_origin(origin) else {
        return;
    };
    if !allowed
        .iter()
        .any(|o| parse_origin(o).as_deref() == Some(parsed.as_str()))
    {
        return;
    }
    if let Ok(v) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    if preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type"),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from(PREFLIGHT_MAX_AGE),
        );
    }
}

/// CORS 中间件服务：包装内层服务，按配置放行跨域 API 请求。
#[derive(Clone)]
pub struct CorsService<S> {
    inner: S,
    allowed_origins: std::sync::Arc<Vec<String>>,
    api_prefix: &'static str,
}

impl<S> tower::Service<Request<axum::body::Body>> for CorsService<S>
where
    S: tower::Service<Request<axum::body::Body>, Response = axum::response::Response>
        + Clone
        + Send
        + 'static,
    S::Future: Send,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<axum::body::Body>) -> Self::Future {
        // 仅 API 路径参与 CORS（对齐上游 api_path 前缀判断）。
        if !req.uri().path().starts_with(self.api_prefix) {
            let mut inner = self.inner.clone();
            return Box::pin(async move { inner.call(req).await });
        }
        let preflight = is_preflight(&req);
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let allowed = self.allowed_origins.clone();
        let mut inner = self.inner.clone();
        Box::pin(async move {
            // 预检请求直接短路应答（204，无 body），避免进入认证层。
            let mut resp: axum::response::Response = if preflight {
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::NO_CONTENT)
                    .body(axum::body::Body::empty())
                    .expect("静态预检响应构造失败")
            } else {
                inner.call(req).await?
            };
            cors_headers(resp.headers_mut(), origin.as_deref(), &allowed, preflight);
            Ok(resp)
        })
    }
}

/// CORS 中间件 layer：`allowed_origins` 为空时退化为原样透传（中间件自移除语义）。
#[derive(Clone)]
pub struct CorsLayer {
    allowed_origins: std::sync::Arc<Vec<String>>,
}

impl CorsLayer {
    pub fn new(allowed_origins: &[String]) -> Self {
        Self {
            allowed_origins: std::sync::Arc::new(allowed_origins.to_vec()),
        }
    }
}

impl<S> tower::Layer<S> for CorsLayer {
    type Service = CorsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        CorsService {
            inner,
            allowed_origins: self.allowed_origins.clone(),
            api_prefix: "/api/",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_origin_accepts_scheme_host_port() {
        assert_eq!(
            parse_origin("https://example.com").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            parse_origin("http://LocalHost:3000").as_deref(),
            Some("http://localhost:3000")
        );
    }

    #[test]
    fn parse_origin_rejects_bad_input() {
        assert_eq!(parse_origin(""), None);
        assert_eq!(parse_origin("example.com"), None);
        assert_eq!(parse_origin("ftp://example.com"), None);
        assert_eq!(parse_origin("https://example.com/path"), None);
    }

    #[test]
    fn cors_headers_only_for_allowed_origin() {
        let allowed = vec!["https://app.example.com".to_string()];
        let mut headers = axum::http::HeaderMap::new();
        cors_headers(
            &mut headers,
            Some("https://app.example.com"),
            &allowed,
            false,
        );
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).unwrap(),
            "https://app.example.com"
        );
        assert_eq!(headers.get(header::ACCESS_CONTROL_ALLOW_METHODS), None);

        let mut headers = axum::http::HeaderMap::new();
        cors_headers(
            &mut headers,
            Some("https://evil.example.com"),
            &allowed,
            false,
        );
        assert!(headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    }

    #[test]
    fn cors_headers_preflight_adds_methods() {
        let allowed = vec!["https://app.example.com".to_string()];
        let mut headers = axum::http::HeaderMap::new();
        cors_headers(
            &mut headers,
            Some("https://app.example.com"),
            &allowed,
            true,
        );
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_METHODS).unwrap(),
            "GET, POST, PUT, PATCH, DELETE, OPTIONS"
        );
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_HEADERS).unwrap(),
            "Authorization, Content-Type"
        );
    }
}
