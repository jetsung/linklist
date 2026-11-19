use serde::Deserialize;

/// Internet Archive 可用性查询端点（可注入以便测试）。
pub const DEFAULT_AVAILABILITY_URL: &str = "https://archive.org/wayback/available";

/// availability API 响应。
#[derive(Debug, Deserialize)]
struct AvailabilityResponse {
    #[serde(default)]
    archived_snapshots: ArchivedSnapshots,
}

#[derive(Debug, Deserialize, Default)]
struct ArchivedSnapshots {
    #[serde(default)]
    closest: Option<ClosestSnapshot>,
}

#[derive(Debug, Deserialize)]
struct ClosestSnapshot {
    url: String,
    #[serde(default)]
    status: Option<String>,
}

/// 查询结果。
#[derive(Debug, Clone, PartialEq)]
pub enum ArchiveLookup {
    /// 找到最近快照。
    Found(String),
    /// 无快照。
    NotFound,
}

/// 查询 Internet Archive 是否有目标 URL 的存档（便捷封装；测试与嵌入场景使用）。
#[allow(dead_code)]
pub async fn lookup(base_url: &str, target_url: &str) -> Result<ArchiveLookup, String> {
    lookup_with(base_url, target_url, &reqwest::Client::new()).await
}

/// 可注入 HTTP 客户端的查询。
pub async fn lookup_with(
    base_url: &str,
    target_url: &str,
    client: &reqwest::Client,
) -> Result<ArchiveLookup, String> {
    let resp = client
        .get(base_url)
        .query(&[("url", target_url)])
        .send()
        .await
        .map_err(|e| format!("Archive 查询失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Archive 返回非 2xx: {}", resp.status()));
    }
    let parsed: AvailabilityResponse = resp
        .json()
        .await
        .map_err(|e| format!("Archive 响应解析失败: {e}"))?;
    match parsed.archived_snapshots.closest {
        Some(closest) if closest.status.as_deref() != Some("404") => {
            Ok(ArchiveLookup::Found(closest.url))
        }
        _ => Ok(ArchiveLookup::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::get;

    async fn spawn_mock(router: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn lookup_found() {
        let router = axum::Router::new().route(
            "/wayback/available",
            get(|| async {
                Json(serde_json::json!({
                    "archived_snapshots": {
                        "closest": {"url": "https://web.archive.org/web/2024/https://example.com", "status": "200"}
                    }
                }))
            }),
        );
        let base = spawn_mock(router).await;
        let r = lookup(&format!("{base}/wayback/available"), "https://example.com")
            .await
            .unwrap();
        assert_eq!(
            r,
            ArchiveLookup::Found("https://web.archive.org/web/2024/https://example.com".into())
        );
    }

    #[tokio::test]
    async fn lookup_not_found() {
        let router = axum::Router::new().route(
            "/wayback/available",
            get(|| async { Json(serde_json::json!({"archived_snapshots": {}})) }),
        );
        let base = spawn_mock(router).await;
        let r = lookup(&format!("{base}/wayback/available"), "https://nope.example")
            .await
            .unwrap();
        assert_eq!(r, ArchiveLookup::NotFound);
    }

    #[tokio::test]
    async fn lookup_query_failure() {
        // 不可达端口。
        let r = lookup(
            "http://127.0.0.1:9/wayback/available",
            "https://example.com",
        )
        .await;
        assert!(r.is_err());
    }
}
