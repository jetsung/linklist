use std::net::IpAddr;
use std::time::Duration;

use reqwest::{Client, Method};

use crate::capture::ssrf;

/// 出站抓取限制。
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024; // 2 MiB
pub const MAX_REDIRECTS: usize = 5;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// 统一出站客户端（超时/重定向/UA 收口）。
pub fn outbound_client() -> Client {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .user_agent(concat!("LinkList/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("构建出站客户端失败")
}

/// SSRF 校验 + 解析目标主机全部 IP 并复核。
pub async fn validate_and_resolve(_client: &Client, raw: &str) -> Result<url::Url, String> {
    let url = ssrf::validate_outbound_url(raw)?;
    let host = url.host_str().ok_or("URL 缺少主机名")?.to_string();
    // IP 字面量已在校验中覆盖；域名需解析（白名单主机在 URL 校验时已放行，
    // 其解析结果同样跳过私网复核，保证内网服务可抓）。
    if host.parse::<IpAddr>().is_err() && !ssrf::host_allowed(&host) {
        let scheme_port = match url.scheme() {
            "https" => 443,
            _ => 80,
        };
        let addrs = tokio::net::lookup_host((host.as_str(), scheme_port))
            .await
            .map_err(|e| format!("DNS 解析失败: {e}"))?;
        let ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
        ssrf::check_resolved_ips(&ips)?;
    }
    Ok(url)
}

/// 受控 GET：SSRF 校验 + 大小上限（超过即中断）。
pub async fn safe_get(
    client: &Client,
    raw_url: &str,
) -> Result<(url::Url, Vec<u8>, reqwest::header::HeaderMap), String> {
    let url = validate_and_resolve(client, raw_url).await?;
    let resp = client
        .request(Method::GET, url.clone())
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("目标返回非 2xx: {}", resp.status()));
    }
    let headers = resp.headers().clone();
    let bytes = read_limited(resp, MAX_RESPONSE_BYTES).await?;
    Ok((url, bytes, headers))
}

/// 流式读取并强制大小上限。
pub async fn read_limited(resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    use futures::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取响应失败: {e}"))?;
        if buf.len() + chunk.len() > max {
            return Err(format!("响应超过大小上限 {max} 字节"));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn safe_get_rejects_private_target() {
        let client = outbound_client();
        let err = safe_get(&client, "http://127.0.0.1:9/x").await.unwrap_err();
        assert!(err.contains("禁止"), "{err}");
    }

    #[tokio::test]
    async fn safe_get_rejects_bad_scheme() {
        let client = outbound_client();
        let err = safe_get(&client, "ftp://example.com").await.unwrap_err();
        assert!(err.contains("不允许的协议"), "{err}");
    }
}
