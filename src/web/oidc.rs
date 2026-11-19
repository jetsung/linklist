//! OIDC（OpenID Connect）授权码登录：对齐上游 `LD_ENABLE_OIDC`（mozilla_django_oidc）。
//!
//! 流程：
//! 1. `GET /oidc/login` → 302 到 IdP 授权端点（带 state/PKCE，写临时会话）。
//! 2. IdP 回跳 `GET /oidc/callback?code&state` → 校验 state，换取 token，
//!    校验 ID Token（RS256 走 JWKS；HS256 走 client_secret），提取用户名 claim，
//!    自动建号并登录。
//!
//! 与上游差异：用户名算法固定为「claim 值原样」（上游 OIDC_USERNAME_ALGO=generate_username
//! 会做 slug 化；此处直接使用 claim 字符串，长度/字符由 claim 本身保证）。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use axum::extract::State;
use axum::response::{IntoResponse, Redirect, Response};
use base64::Engine;
use serde::Deserialize;
use tower_sessions::Session;

use crate::error::AppError;
use crate::models::{self, audit_action};
use crate::web::auth_api::{AppState, SESSION_USER_KEY};

/// OIDC 登录过程中的临时会话键。
const OIDC_STATE_KEY: &str = "oidc_state";
const OIDC_VERIFIER_KEY: &str = "oidc_verifier";
const OIDC_NONCE_KEY: &str = "oidc_nonce";

/// 授权码流程会话临时项的存活时长（秒）。
const FLOW_TTL_SECS: i64 = 600;

pub(crate) fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// 生成 `n` 个随机字节的 base64url 字符串。
fn random_b64(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::fill(&mut bytes);
    b64url(&bytes)
}

/// 解析后的 OIDC 端点集合（显式配置优先，缺失项由 discovery 填充）。
#[derive(Clone, Debug)]
pub(crate) struct OidcEndpoints {
    pub authorization: String,
    pub token: String,
    pub jwks: Option<String>,
    pub userinfo: Option<String>,
}

/// discovery 结果进程内缓存：issuer → 端点集合。
/// 按 issuer 作键，不同 IdP（含测试 mock）互不串扰。
static DISCOVERY_CACHE: OnceLock<RwLock<HashMap<String, Arc<OidcEndpoints>>>> = OnceLock::new();

fn discovery_cache() -> &'static RwLock<HashMap<String, Arc<OidcEndpoints>>> {
    DISCOVERY_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn non_empty(v: Option<&str>) -> Option<String> {
    v.filter(|s| !s.is_empty()).map(str::to_string)
}

/// 解析 OIDC 端点：显式配置优先；缺失项在配置了 Issuer 时从
/// `{issuer}/.well-known/openid-configuration` 自动发现并进程内缓存。
async fn resolve_endpoints(cfg: &crate::config::Config) -> Result<OidcEndpoints, AppError> {
    let explicit = (
        non_empty(cfg.oidc_authorization_endpoint.as_deref()),
        non_empty(cfg.oidc_token_endpoint.as_deref()),
        non_empty(cfg.oidc_jwks_endpoint.as_deref()),
        non_empty(cfg.oidc_userinfo_endpoint.as_deref()),
    );
    if let (Some(a), Some(t)) = (&explicit.0, &explicit.1) {
        return Ok(OidcEndpoints {
            authorization: a.clone(),
            token: t.clone(),
            jwks: explicit.2.clone(),
            userinfo: explicit.3.clone(),
        });
    }

    let issuer = non_empty(cfg.oidc_issuer.as_deref()).ok_or_else(|| {
        AppError::BadRequest("OIDC issuer 未配置：请设置 LINKLIST_OIDC_ISSUER 或各端点 URL".into())
    })?;

    let discovered = {
        let cached = discovery_cache()
            .read()
            .expect("discovery 缓存锁")
            .get(&issuer)
            .cloned();
        match cached {
            Some(d) => d,
            None => {
                let url = format!(
                    "{}/.well-known/openid-configuration",
                    issuer.trim_end_matches('/')
                );
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                    .map_err(|e| {
                        AppError::BadRequest(format!("OIDC discovery 客户端构建失败: {e}"))
                    })?;
                let doc: serde_json::Value = client
                    .get(&url)
                    .send()
                    .await
                    .and_then(|r| r.error_for_status())
                    .map_err(|e| AppError::BadRequest(format!("OIDC discovery 请求失败: {e}")))?
                    .json()
                    .await
                    .map_err(|e| {
                        AppError::BadRequest(format!("OIDC discovery 响应解析失败: {e}"))
                    })?;
                let field = |k: &str| -> Result<String, AppError> {
                    doc.get(k)
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .ok_or_else(|| AppError::BadRequest(format!("OIDC discovery 缺少 {k}")))
                };
                let d = Arc::new(OidcEndpoints {
                    authorization: field("authorization_endpoint")?,
                    token: field("token_endpoint")?,
                    jwks: doc
                        .get("jwks_uri")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    userinfo: doc
                        .get("userinfo_endpoint")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });
                discovery_cache()
                    .write()
                    .expect("discovery 缓存锁")
                    .insert(issuer.to_string(), d.clone());
                d
            }
        }
    };

    Ok(OidcEndpoints {
        authorization: explicit
            .0
            .unwrap_or_else(|| discovered.authorization.clone()),
        token: explicit.1.unwrap_or_else(|| discovered.token.clone()),
        jwks: explicit.2.or_else(|| discovered.jwks.clone()),
        userinfo: explicit.3.or_else(|| discovered.userinfo.clone()),
    })
}

/// `GET /oidc/login`：发起授权码流程。
///
/// 未配置 OIDC → 404（对齐上游中间件自移除语义）。
pub async fn oidc_login(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let cfg = &state.config;
    if !cfg.enable_oidc {
        return Err(AppError::NotFound);
    }
    let endpoints = resolve_endpoints(cfg).await?;
    let auth_endpoint = endpoints.authorization.as_str();
    let client_id = cfg
        .oidc_client_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(AppError::BadRequest("OIDC client_id 未配置".into()))?;

    // state/nonce/verifier 均写入会话并设短 TTL，防 CSRF/重放。
    let state_param = random_b64(24);
    let nonce = random_b64(24);
    let verifier = random_b64(48);
    session
        .insert(
            OIDC_STATE_KEY,
            (state_param.clone(), now_ts() + FLOW_TTL_SECS),
        )
        .await
        .map_err(AppError::internal)?;
    session
        .insert(OIDC_NONCE_KEY, (nonce.clone(), now_ts() + FLOW_TTL_SECS))
        .await
        .map_err(AppError::internal)?;
    session
        .insert(
            OIDC_VERIFIER_KEY,
            (verifier.clone(), now_ts() + FLOW_TTL_SECS),
        )
        .await
        .map_err(AppError::internal)?;

    // PKCE S256：challenge = BASE64URL(SHA256(verifier))。
    use sha2::Digest;
    let challenge = b64url(&sha2::Sha256::digest(verifier.as_bytes()));

    // redirect_uri 按请求 Host 推导（反代/测试场景均可）。
    let base = crate::web::base_url_from_headers(&headers);
    let redirect_uri = format!("{base}/oidc/callback");

    let mut url = format!(
        "{auth_endpoint}?response_type=code&client_id={client_id}&state={state_param}&nonce={nonce}&redirect_uri={redirect_uri}&scope=openid%20email%20profile&code_challenge={challenge}&code_challenge_method=S256"
    );
    if cfg.oidc_sign_algo.is_empty() {
        url.push_str("&alg=RS256");
    }
    Ok(Redirect::to(&url).into_response())
}

#[derive(Deserialize)]
pub struct OidcCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

/// `GET /oidc/callback`：处理 IdP 回跳，完成登录。
pub async fn oidc_callback(
    State(state): State<AppState>,
    session: Session,
    headers: axum::http::HeaderMap,
    axum::extract::Query(q): axum::extract::Query<OidcCallbackQuery>,
) -> Result<Response, AppError> {
    let cfg = &state.config;
    if !cfg.enable_oidc {
        return Err(AppError::NotFound);
    }
    if let Some(err) = &q.error {
        return Err(AppError::BadRequest(format!("OIDC 授权失败: {err}")));
    }
    let code = q
        .code
        .clone()
        .ok_or_else(|| AppError::BadRequest("缺少 code 参数".into()))?;

    // 校验 state（CSRF 防护）并消费。
    let Some((saved_state, expires)) = session
        .get::<(String, i64)>(OIDC_STATE_KEY)
        .await
        .map_err(AppError::internal)?
    else {
        return Err(AppError::BadRequest("OIDC state 缺失或已过期".into()));
    };
    let _ = session.remove::<(String, i64)>(OIDC_STATE_KEY).await;
    if now_ts() > expires {
        return Err(AppError::BadRequest("OIDC state 已过期".into()));
    }
    if q.state.as_deref() != Some(saved_state.as_str()) {
        return Err(AppError::BadRequest("OIDC state 不匹配".into()));
    }

    let endpoints = resolve_endpoints(cfg).await?;
    let token_endpoint = endpoints.token.as_str();
    let client_id = cfg
        .oidc_client_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or(AppError::BadRequest("OIDC client_id 未配置".into()))?;
    let client_secret = cfg.oidc_client_secret.as_deref().unwrap_or_default();
    let base = crate::web::base_url_from_headers(&headers);
    let redirect_uri = format!("{base}/oidc/callback");

    let verifier: Option<String> = session
        .get::<(String, i64)>(OIDC_VERIFIER_KEY)
        .await
        .map_err(AppError::internal)?
        .map(|(v, _)| v);
    let _ = session.remove::<(String, i64)>(OIDC_VERIFIER_KEY).await;

    // 换取 token。
    let client = reqwest::Client::new();
    let mut params = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("client_id", client_id.to_string()),
        ("redirect_uri", redirect_uri),
    ];
    if !client_secret.is_empty() {
        params.push(("client_secret", client_secret.to_string()));
    }
    if let Some(v) = &verifier {
        params.push(("code_verifier", v.clone()));
    }
    let token_resp: TokenResponse = client
        .post(token_endpoint)
        .form(&params)
        .send()
        .await
        .map_err(|e| AppError::BadRequest(format!("OIDC token 请求失败: {e}")))?
        .error_for_status()
        .map_err(|e| AppError::BadRequest(format!("OIDC token 请求被拒绝: {e}")))?
        .json()
        .await
        .map_err(|e| AppError::BadRequest(format!("OIDC token 响应解析失败: {e}")))?;

    // 解析用户名：优先 ID Token claim，其次 userinfo 端点。
    let mut claims = serde_json::Map::new();
    if let Some(id_token) = &token_resp.id_token {
        let verified = verify_id_token(
            id_token,
            client_id,
            cfg.oidc_sign_algo.as_str(),
            client_secret,
            endpoints.jwks.as_deref(),
            &client,
        )
        .await?;
        claims = verified;
    }
    if !claims.contains_key("sub") {
        // ID Token 缺失/不可验时退回 userinfo（仍要求 access_token）。
        let userinfo_endpoint = endpoints
            .userinfo
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("OIDC 未返回可验证的 ID Token".into()))?;
        let access_token = token_resp
            .access_token
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("OIDC 未返回 access_token".into()))?;
        let userinfo: serde_json::Value = client
            .get(userinfo_endpoint)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| AppError::BadRequest(format!("OIDC userinfo 请求失败: {e}")))?
            .error_for_status()
            .map_err(|e| AppError::BadRequest(format!("OIDC userinfo 被拒绝: {e}")))?
            .json()
            .await
            .map_err(|e| AppError::BadRequest(format!("OIDC userinfo 解析失败: {e}")))?;
        if let Some(obj) = userinfo.as_object() {
            claims = obj.clone();
        }
    }

    let username_claim = cfg.oidc_username_claim.as_str();
    let username = claims
        .get(username_claim)
        .and_then(|v| v.as_str())
        .or_else(|| claims.get("sub").and_then(|v| v.as_str()))
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AppError::BadRequest(format!("OIDC 响应缺少用户名 claim: {username_claim}"))
        })?;

    // 自动建号 + 登录（对齐 OIDC_USERNAME_ALGO 语义：以 claim 值为用户名）。
    let user = match models::find_by_username(&state.pool, &username).await? {
        Some(u) if u.is_active => u,
        Some(_) => return Err(AppError::Forbidden),
        None => models::create_user(&state.pool, &username, &random_password(), false).await?,
    };
    session
        .insert(SESSION_USER_KEY, user.id)
        .await
        .map_err(AppError::internal)?;
    crate::models::audit(&state.pool, Some(user.id), audit_action::LOGIN, "oidc").await;

    // 登录成功 → 跳首页。
    Ok(Redirect::to("/").into_response())
}

/// 生成占位随机口令（OIDC 用户不走密码登录）。
fn random_password() -> String {
    random_b64(24)
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 验证 ID Token 签名并返回 claims。
///
/// - RS256：从 JWKS 端点取公钥验签（RSA SHA-256）。
/// - HS256：用 client_secret 做 HMAC-SHA256 验签。
async fn verify_id_token(
    id_token: &str,
    client_id: &str,
    algo: &str,
    client_secret: &str,
    jwks_endpoint: Option<&str>,
    client: &reqwest::Client,
) -> Result<serde_json::Map<String, serde_json::Value>, AppError> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return Err(AppError::BadRequest("ID Token 格式非法".into()));
    }
    let (header_b64, payload_b64, sig_b64) = (parts[0], parts[1], parts[2]);
    let header_json = decode_segment(header_b64)?;
    let payload_json = decode_segment(payload_b64)?;
    let header: serde_json::Value = serde_json::from_slice(&header_json)
        .map_err(|e| AppError::BadRequest(format!("ID Token header 解析失败: {e}")))?;
    let token_algo = header
        .get("alg")
        .and_then(|v| v.as_str())
        .unwrap_or("RS256")
        .to_string();
    if !token_algo.eq_ignore_ascii_case(algo) {
        return Err(AppError::BadRequest(format!(
            "ID Token 算法不匹配: {token_algo} != {algo}"
        )));
    }

    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|e| AppError::BadRequest(format!("ID Token 签名解码失败: {e}")))?;
    let signed_input = format!("{header_b64}.{payload_b64}");

    match token_algo.to_ascii_uppercase().as_str() {
        "HS256" => {
            use hmac::{Hmac, KeyInit, Mac};
            type HmacSha256 = Hmac<sha2::Sha256>;
            let mut mac =
                HmacSha256::new_from_slice(client_secret.as_bytes()).map_err(AppError::internal)?;
            mac.update(signed_input.as_bytes());
            let expected = mac.finalize().into_bytes();
            if expected.as_slice() != signature.as_slice() {
                return Err(AppError::BadRequest("ID Token 签名校验失败".into()));
            }
        }
        "RS256" => {
            let jwks = jwks_endpoint
                .filter(|s| !s.is_empty())
                .ok_or_else(|| AppError::BadRequest("OIDC JWKS 端点未配置".into()))?;
            let jwks: serde_json::Value = client
                .get(jwks)
                .send()
                .await
                .map_err(|e| AppError::BadRequest(format!("JWKS 请求失败: {e}")))?
                .error_for_status()
                .map_err(|e| AppError::BadRequest(format!("JWKS 被拒绝: {e}")))?
                .json()
                .await
                .map_err(|e| AppError::BadRequest(format!("JWKS 解析失败: {e}")))?;
            let kid = header.get("kid").and_then(|v| v.as_str());
            let (n_b64, e_b64) = find_rsa_key(&jwks, kid, &token_algo)?;
            let key = rsa_key_from_jwk(&n_b64, &e_b64)?;
            verify_rsa_sha256(&key, signed_input.as_bytes(), &signature)?;
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "不支持的 ID Token 算法: {other}"
            )));
        }
    }

    let claims: serde_json::Value = serde_json::from_slice(&payload_json)
        .map_err(|e| AppError::BadRequest(format!("ID Token payload 解析失败: {e}")))?;
    let claims = claims
        .as_object()
        .cloned()
        .ok_or_else(|| AppError::BadRequest("ID Token payload 不是对象".into()))?;

    // aud 校验（对齐上游 OIDC_RP_CLIENT_ID）。
    let aud_ok = match claims.get("aud") {
        Some(serde_json::Value::String(s)) => s == client_id,
        Some(serde_json::Value::Array(arr)) => arr.iter().any(|v| v.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err(AppError::BadRequest("ID Token aud 不匹配".into()));
    }
    // exp 校验（存在时）。
    if let Some(exp) = claims.get("exp").and_then(|v| v.as_i64())
        && now_ts() > exp
    {
        return Err(AppError::BadRequest("ID Token 已过期".into()));
    }
    Ok(claims)
}

fn decode_segment(seg: &str) -> Result<Vec<u8>, AppError> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(seg)
        .map_err(|e| AppError::BadRequest(format!("ID Token 段解码失败: {e}")))
}

/// 从 JWKS 中找出匹配 kid 的 RSA 公钥参数（n, e）。
fn find_rsa_key(
    jwks: &serde_json::Value,
    kid: Option<&str>,
    algo: &str,
) -> Result<(String, String), AppError> {
    let keys = jwks
        .get("keys")
        .and_then(|v| v.as_array())
        .ok_or_else(|| AppError::BadRequest("JWKS 缺少 keys 数组".into()))?;
    for key in keys {
        let kty = key.get("kty").and_then(|v| v.as_str()).unwrap_or("");
        if !kty.eq_ignore_ascii_case("RSA") {
            continue;
        }
        if let Some(kid) = kid {
            let this_kid = key.get("kid").and_then(|v| v.as_str()).unwrap_or("");
            if this_kid != kid {
                continue;
            }
        }
        if let Some(alg) = key.get("alg").and_then(|v| v.as_str())
            && !alg.eq_ignore_ascii_case(algo)
        {
            continue;
        }
        let n = key
            .get("n")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::BadRequest("JWKS RSA 密钥缺少 n".into()))?;
        let e = key
            .get("e")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::BadRequest("JWKS RSA 密钥缺少 e".into()))?;
        return Ok((n.to_string(), e.to_string()));
    }
    Err(AppError::BadRequest("JWKS 中未找到匹配的 RSA 密钥".into()))
}

/// 从 JWK 的 n/e（base64url 大端）构造 RSA 公钥。
fn rsa_key_from_jwk(n_b64: &str, e_b64: &str) -> Result<rsa::PublicKey, AppError> {
    use rsa::BoxedUint;
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(n_b64)
        .map_err(|e| AppError::BadRequest(format!("RSA n 解码失败: {e}")))?;
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(e_b64)
        .map_err(|e| AppError::BadRequest(format!("RSA e 解码失败: {e}")))?;
    rsa::PublicKey::new(
        BoxedUint::from_be_slice(&n, (n.len() * 8) as u32)
            .map_err(|err| AppError::BadRequest(format!("RSA n 非法: {err}")))?,
        BoxedUint::from_be_slice(&e, (e.len() * 8) as u32)
            .map_err(|err| AppError::BadRequest(format!("RSA e 非法: {err}")))?,
    )
    .map_err(|err| AppError::BadRequest(format!("RSA 公钥构造失败: {err}")))
}

/// RSA PKCS#1 v1.5 + SHA-256 验签。
fn verify_rsa_sha256(key: &rsa::PublicKey, msg: &[u8], signature: &[u8]) -> Result<(), AppError> {
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::sha2::Sha256;
    use rsa::signature::Verifier;
    let verifying_key: VerifyingKey<Sha256> = VerifyingKey::new(key.clone());
    let sig = Signature::try_from(signature)
        .map_err(|e| AppError::BadRequest(format!("签名长度非法: {e}")))?;
    verifying_key
        .verify(msg, &sig)
        .map_err(|_| AppError::BadRequest("ID Token 签名校验失败".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_b64_produces_urlsafe() {
        let s = random_b64(32);
        assert_eq!(s.len(), 43); // 32 bytes → 43 chars unpadded
        assert!(
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn decode_segment_roundtrip() {
        let encoded = b64url(b"hello");
        assert_eq!(decode_segment(&encoded).unwrap(), b"hello");
    }

    #[test]
    fn find_rsa_key_matches_kid() {
        let jwks = serde_json::json!({
            "keys": [
                {"kty":"RSA","kid":"k1","alg":"RS256","n":"abc","e":"AQAB"},
                {"kty":"RSA","kid":"k2","alg":"RS256","n":"def","e":"AQAB"},
            ]
        });
        let (n, _) = find_rsa_key(&jwks, Some("k2"), "RS256").unwrap();
        assert_eq!(n, "def");
        let (n, _) = find_rsa_key(&jwks, None, "RS256").unwrap();
        assert_eq!(n, "abc");
        assert!(find_rsa_key(&jwks, Some("missing"), "RS256").is_err());
    }

    #[tokio::test]
    async fn oidc_login_disabled_returns_404() {
        use tower::ServiceExt;
        let pool = crate::db::connect_in_memory().await.unwrap();
        let app = crate::web::auth_api::integration::build_app_with_pool(pool, None)
            .await
            .0;
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/oidc/login")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::NOT_FOUND);
    }
}
