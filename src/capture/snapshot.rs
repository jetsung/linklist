use std::path::{Path, PathBuf};

use crate::capture::http::{outbound_client, safe_get};

/// 快照目录：{data_dir}/snapshots。
pub fn snapshots_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("snapshots")
}

/// 快照文件路径：{data_dir}/snapshots/{bookmark_id}.html。
pub fn snapshot_path(data_dir: &Path, bookmark_id: i64) -> PathBuf {
    snapshots_dir(data_dir).join(format!("{bookmark_id}.html"))
}

/// 资源抓取注入点：返回 (mime, bytes)。生产走 HTTP；测试可注入本地 mock。
pub type ResourceFetcher = std::sync::Arc<
    dyn Fn(String) -> futures::future::BoxFuture<'static, Result<(String, Vec<u8>), String>>
        + Send
        + Sync,
>;

/// 生产抓取实现。
pub fn http_fetcher() -> ResourceFetcher {
    std::sync::Arc::new(|url| {
        Box::pin(async move {
            let client = outbound_client();
            let (_u, bytes, headers) = safe_get(&client, &url).await?;
            let mime = headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            Ok((mime, bytes))
        })
    })
}

/// 生成 singlefile 风格快照：抓页面 → CSS/图片内联、相对路径转绝对 → 写入数据目录并返回路径（便捷封装）。
#[allow(dead_code)]
pub async fn capture_snapshot(
    data_dir: &Path,
    bookmark_id: i64,
    url: &str,
) -> Result<PathBuf, String> {
    capture_snapshot_with(data_dir, bookmark_id, url, http_fetcher()).await
}

/// 可注入抓取器的快照生成。
pub async fn capture_snapshot_with(
    data_dir: &Path,
    bookmark_id: i64,
    url: &str,
    fetcher: ResourceFetcher,
) -> Result<PathBuf, String> {
    let (mime, html_bytes) = fetcher(url.to_string()).await?;
    let _ = mime;
    let base = url::Url::parse(url).map_err(|e| format!("URL 解析失败: {e}"))?;
    let html = String::from_utf8_lossy(&html_bytes);
    let inlined = inline_resources_with(&base, &html, fetcher).await;

    let path = snapshot_path(data_dir, bookmark_id);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("创建快照目录失败: {e}"))?;
    }
    tokio::fs::write(&path, &inlined)
        .await
        .map_err(|e| format!("写快照失败: {e}"))?;
    Ok(path)
}

/// 内联页面资源：`<link rel=stylesheet>` → `<style>`；`<img src>` → 绝对 URL 或 data: URL。
/// 基于标签扫描（不用 DOM 偏移），对畸形 HTML 保守处理。
#[allow(dead_code)]
pub async fn inline_resources(base: &url::Url, html: &str) -> String {
    inline_resources_with(base, html, http_fetcher()).await
}

/// 可注入抓取器的内联实现。
pub async fn inline_resources_with(
    base: &url::Url,
    html: &str,
    fetcher: ResourceFetcher,
) -> String {
    let mut out = html.to_string();
    out = inline_stylesheets(base, &out, &fetcher).await;
    out = inline_images(base, &out, &fetcher).await;
    out
}

/// 把 `<link rel="stylesheet" href="...">` 替换为 `<style>...</style>`（抓取失败则保留原标签）。
async fn inline_stylesheets(base: &url::Url, html: &str, fetcher: &ResourceFetcher) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut last = 0usize;
    let _bytes = html.as_bytes();
    let _lower_bytes = lower.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("<link") {
        let tag_start = i + rel;
        let tag_end = match lower[tag_start..].find('>') {
            Some(p) => tag_start + p + 1,
            None => break,
        };
        let tag = &html[tag_start..tag_end];
        let tag_lower = &lower[tag_start..tag_end];
        out.push_str(&html[last..tag_start]);
        last = tag_end;
        i = tag_end;
        // 仅处理 stylesheet。
        if !(tag_lower.contains("rel=\"stylesheet\"")
            || tag_lower.contains("rel='stylesheet'")
            || tag_lower.contains("rel=stylesheet"))
        {
            continue;
        }
        let Some(href) = extract_attr(tag, "href") else {
            continue;
        };
        let abs = match base.join(&href) {
            Ok(u) => u,
            Err(_) => continue,
        };
        match fetch_via(fetcher, abs.as_str()).await {
            Ok((mime, bytes)) => {
                let css = String::from_utf8_lossy(&bytes);
                let _ = mime;
                out.push_str(&format!(
                    "<style>/* inlined from {href} */\n{css}\n</style>"
                ));
                // 跳过原标签（已替换）。
                let _ = bytes;
            }
            Err(_) => {
                // 抓取失败：保留原标签。
                out.push_str(tag);
            }
        }
    }
    out.push_str(&html[last..]);
    out
}

/// 把 `<img src="...">` 的 src 替换为 data: URL（抓取成功）或绝对 URL（失败兜底）。
async fn inline_images(base: &url::Url, html: &str, fetcher: &ResourceFetcher) -> String {
    let lower = html.to_ascii_lowercase();
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0usize;
    while let Some(pos) = lower[i..].find("<img") {
        let tag_start = i + pos;
        let tag_end = match lower[tag_start..].find('>') {
            Some(p) => tag_start + p + 1,
            None => break,
        };
        let tag = &html[tag_start..tag_end];
        if let Some(src) = extract_attr(tag, "src")
            && !src.starts_with("data:")
            && let Ok(abs) = base.join(&src)
        {
            let replacement = match fetch_via(fetcher, abs.as_str()).await {
                Ok((mime, data)) => format!(
                    "data:{mime};base64,{}",
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data)
                ),
                Err(_) => abs.to_string(),
            };
            if let Some((vs, ve)) = find_attr_value_range(tag, "src", &src) {
                replacements.push((tag_start + vs, tag_start + ve, replacement));
            }
        }
        i = tag_end;
    }
    let mut out = String::with_capacity(html.len());
    let mut last = 0usize;
    for (s, e, text) in replacements {
        if s < last {
            continue;
        }
        out.push_str(&html[last..s]);
        out.push_str(&text);
        last = e;
    }
    out.push_str(&html[last..]);
    out
}

/// 从标签文本中提取属性值（支持双引号/单引号/裸值）。
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let pat = format!("{attr}=");
    let pos = lower.find(&pat)?;
    let rest = &tag[pos + pat.len()..];
    let rest = rest.trim_start();
    let (value, _) = read_quoted_or_bare(rest)?;
    Some(value)
}

/// 找属性值在标签内的字节范围 [start,end)。
fn find_attr_value_range(tag: &str, attr: &str, value: &str) -> Option<(usize, usize)> {
    let lower = tag.to_ascii_lowercase();
    let pat = format!("{attr}=");
    let pos = lower.find(&pat)?;
    let after = pos + pat.len();
    let rest = &tag[after..];
    let trimmed = rest.len() - rest.trim_start().len();
    let (v, consumed) = read_quoted_or_bare(&rest[trimmed..])?;
    if v != value {
        return None;
    }
    let vstart = after + trimmed + consumed
        - v.len()
        - if rest.trim_start().starts_with('"') || rest.trim_start().starts_with('\'') {
            1
        } else {
            0
        };
    Some((vstart, vstart + v.len()))
}

/// 读取引号包裹或裸属性值，返回 (值, 消耗字节数)。
fn read_quoted_or_bare(s: &str) -> Option<(String, usize)> {
    let mut chars = s.char_indices();
    let (_, first) = chars.next()?;
    match first {
        '"' | '\'' => {
            let quote = first;
            let end = s[1..].find(quote)? + 1;
            Some((s[1..end].to_string(), end + 1))
        }
        _ => {
            let end = s.find([' ', '>', '\t', '\n']).unwrap_or(s.len());
            Some((s[..end].to_string(), end))
        }
    }
}

/// 统一资源抓取入口。
async fn fetch_via(fetcher: &ResourceFetcher, url: &str) -> Result<(String, Vec<u8>), String> {
    fetcher(url.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn inline_absolutizes_unreachable_images() {
        let base = url::Url::parse("https://example.com/page").unwrap();
        let html = r#"<html><body><img src="/img/a.png"><img src="b.png"></body></html>"#;
        let out = inline_resources(&base, html).await;
        assert!(out.contains("https://example.com/img/a.png"), "{out}");
        assert!(out.contains("https://example.com/b.png"), "{out}");
    }

    #[tokio::test]
    async fn inline_keeps_unreachable_stylesheet_tag() {
        let base = url::Url::parse("https://example.com").unwrap();
        let html =
            r#"<html><head><link rel="stylesheet" href="/style.css"></head><body>x</body></html>"#;
        let out = inline_resources(&base, html).await;
        // 不可达 → 保留原 link 标签。
        assert!(out.contains("<link"), "{out}");
    }

    #[tokio::test]
    async fn inline_data_uri_skipped() {
        let base = url::Url::parse("https://example.com").unwrap();
        let html = r#"<img src="data:image/png;base64,AAA">"#;
        let out = inline_resources(&base, html).await;
        assert!(out.contains("data:image/png;base64,AAA"), "{out}");
    }

    #[test]
    fn extract_attr_variants() {
        assert_eq!(
            extract_attr(r#"<a href="/x">"#, "href").as_deref(),
            Some("/x")
        );
        assert_eq!(extract_attr("<a href='/x'>", "href").as_deref(), Some("/x"));
        assert_eq!(extract_attr("<a href=/x>", "href").as_deref(), Some("/x"));
        assert_eq!(extract_attr("<a>", "href"), None);
    }

    #[test]
    fn snapshot_path_layout() {
        let dir = Path::new("/tmp/ll-data");
        assert_eq!(
            snapshot_path(dir, 7),
            PathBuf::from("/tmp/ll-data/snapshots/7.html")
        );
    }

    #[tokio::test]
    async fn capture_snapshot_unreachable_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let r = capture_snapshot(tmp.path(), 1, "http://127.0.0.1:9/x").await;
        assert!(r.is_err());
    }
}
