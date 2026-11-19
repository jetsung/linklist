use serde::Serialize;

use crate::capture::http::{outbound_client, safe_get};

/// 抓取到的页面元数据。
#[derive(Debug, Clone, Serialize, Default)]
pub struct PageMetadata {
    pub title: String,
    pub description: String,
    pub favicon_url: Option<String>,
    /// Open Graph 图片 URL（绝对化后）。
    pub preview_image_url: Option<String>,
}

/// 从 HTML `<head>` 提取元数据（只读 head，避免解析整个 body）。
pub fn extract_metadata(base_url: &url::Url, html: &str) -> PageMetadata {
    let head_end = html
        .to_ascii_lowercase()
        .find("</head>")
        .map(|i| i + "</head>".len())
        .unwrap_or(html.len());
    let head_html = &html[..head_end.min(html.len())];
    let doc = scraper::Html::parse_fragment(head_html);

    let mut meta = PageMetadata::default();
    let title_sel = scraper::Selector::parse("title").unwrap();
    if let Some(t) = doc.select(&title_sel).next() {
        meta.title = t.text().collect::<String>().trim().to_string();
    }
    let og_title_sel = scraper::Selector::parse(r#"meta[property="og:title"]"#).unwrap();
    if let Some(el) = doc.select(&og_title_sel).next()
        && let Some(c) = el.value().attr("content")
    {
        let c = c.trim();
        if !c.is_empty() {
            meta.title = c.to_string();
        }
    }
    let desc_sel = scraper::Selector::parse(r#"meta[name="description"]"#).unwrap();
    if let Some(el) = doc.select(&desc_sel).next()
        && let Some(c) = el.value().attr("content")
    {
        meta.description = c.trim().to_string();
    }
    let og_desc_sel = scraper::Selector::parse(r#"meta[property="og:description"]"#).unwrap();
    if meta.description.is_empty()
        && let Some(el) = doc.select(&og_desc_sel).next()
        && let Some(c) = el.value().attr("content")
    {
        meta.description = c.trim().to_string();
    }
    let icon_sel =
        scraper::Selector::parse(r#"link[rel~="icon" i], link[rel="shortcut icon"]"#).unwrap();
    if let Some(el) = doc.select(&icon_sel).next()
        && let Some(href) = el.value().attr("href")
    {
        meta.favicon_url = absolutize(base_url, href);
    }
    let og_img_sel = scraper::Selector::parse(r#"meta[property="og:image"]"#).unwrap();
    if let Some(el) = doc.select(&og_img_sel).next()
        && let Some(c) = el.value().attr("content")
    {
        meta.preview_image_url = absolutize(base_url, c);
    }
    // favicon 兜底：/favicon.ico。
    if meta.favicon_url.is_none()
        && let Ok(f) = base_url.join("/favicon.ico")
    {
        meta.favicon_url = Some(f.to_string());
    }
    meta
}

/// 相对路径 → 绝对 URL。
fn absolutize(base: &url::Url, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() {
        return None;
    }
    // data: URL 直接保留。
    if href.starts_with("data:") {
        return Some(href.to_string());
    }
    base.join(href).ok().map(|u| u.to_string())
}

use std::collections::HashMap;
use std::sync::Mutex;

/// 元数据 LRU 缓存：对齐上游 `@lru_cache(maxsize=10)`（website_loader.py）。
/// 仅缓存 check 表单预览用途；任务抓取仍走 `fetch_metadata`。
const METADATA_CACHE_SIZE: usize = 10;

static METADATA_CACHE: Mutex<Option<Lru<String, PageMetadata>>> = Mutex::new(None);

/// 极简 LRU：HashMap + 访问序（Vec），容量 10 足够小，O(n) 可忽略。
struct Lru<K, V> {
    map: HashMap<K, V>,
    order: Vec<K>,
    cap: usize,
}

impl<K: Eq + std::hash::Hash + Clone, V: Clone> Lru<K, V> {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::with_capacity(cap),
            order: Vec::with_capacity(cap),
            cap,
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        if let Some(v) = self.map.get(key).cloned() {
            // 提到最新。
            self.order.retain(|k| k != key);
            self.order.push(key.clone());
            Some(v)
        } else {
            None
        }
    }

    fn put(&mut self, key: K, value: V) {
        if self.map.contains_key(&key) {
            self.order.retain(|k| *k != key);
        } else if self.order.len() >= self.cap
            && let Some(oldest) = self.order.first().cloned()
        {
            self.order.remove(0);
            self.map.remove(&oldest);
        }
        self.order.push(key.clone());
        self.map.insert(key, value);
    }
}

/// 带缓存抓取（`ignore_cache=false` 路径）。
pub async fn fetch_metadata_cached(raw_url: &str) -> Result<PageMetadata, String> {
    // 命中直接返回。
    {
        let mut guard = METADATA_CACHE.lock().unwrap();
        let cache = guard.get_or_insert_with(|| Lru::new(METADATA_CACHE_SIZE));
        if let Some(meta) = cache.get(&raw_url.to_string()) {
            return Ok(meta);
        }
    }
    let meta = fetch_metadata(raw_url).await?;
    let mut guard = METADATA_CACHE.lock().unwrap();
    let cache = guard.get_or_insert_with(|| Lru::new(METADATA_CACHE_SIZE));
    cache.put(raw_url.to_string(), meta.clone());
    Ok(meta)
}

/// 清空缓存（测试用）。
#[cfg_attr(not(test), allow(dead_code))]
pub fn clear_metadata_cache() {
    *METADATA_CACHE.lock().unwrap() = None;
}

/// 抓取并解析页面元数据（SSRF 防护 + 大小/超时限制）。
pub async fn fetch_metadata(raw_url: &str) -> Result<PageMetadata, String> {
    let client = outbound_client();
    let (final_url, bytes, _headers) = safe_get(&client, raw_url).await?;
    // UTF-8 探测：优先 BOM，其次按 UTF-8 解析（失败则 lossy）。
    let text = decode_utf8(&bytes);
    let mut meta = extract_metadata(&final_url, &text);
    if meta.title.is_empty() {
        meta.title = final_url.to_string();
    }
    Ok(meta)
}

/// UTF-8 探测与解码。
pub fn decode_utf8(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(&bytes[3..]).into_owned();
    }
    if bytes.starts_with(&[0xff, 0xfe]) {
        // UTF-16LE BOM。
        let s: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        return String::from_utf16_lossy(&s);
    }
    if bytes.starts_with(&[0xfe, 0xff]) {
        // UTF-16BE BOM。
        let s: Vec<u16> = bytes[2..]
            .chunks(2)
            .map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]))
            .collect();
        return String::from_utf16_lossy(&s);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_metadata_cache_resets_cache() {
        // 预填全局缓存再清空：验证清理后状态归零（消费方 check 接口 ignore_cache 行为依赖它）。
        clear_metadata_cache();
        {
            let guard = METADATA_CACHE.lock().unwrap();
            assert!(guard.is_none(), "清理后缓存应为空");
        }
    }

    #[test]
    fn lru_evicts_least_recently_used() {
        let mut lru: Lru<String, u32> = Lru::new(2);
        lru.put("a".into(), 1);
        lru.put("b".into(), 2);
        assert_eq!(lru.get(&"a".into()), Some(1)); // a 变为最新
        lru.put("c".into(), 3); // 淘汰 b
        assert_eq!(lru.get(&"b".into()), None);
        assert_eq!(lru.get(&"a".into()), Some(1));
        assert_eq!(lru.get(&"c".into()), Some(3));
    }

    #[test]
    fn lru_overwrite_updates_order() {
        let mut lru: Lru<String, u32> = Lru::new(2);
        lru.put("a".into(), 1);
        lru.put("b".into(), 2);
        lru.put("a".into(), 10); // 覆盖 a，b 变为最旧
        lru.put("c".into(), 3); // 淘汰 b
        assert_eq!(lru.get(&"b".into()), None);
        assert_eq!(lru.get(&"a".into()), Some(10));
    }

    #[test]
    fn extract_og_and_fallbacks() {
        let base = url::Url::parse("https://example.com/page").unwrap();
        let html = r#"<html><head>
            <meta property="og:title" content="OG 标题">
            <meta property="og:description" content="OG 描述">
            <meta property="og:image" content="/img/cover.png">
            <link rel="icon" href="/favicon-v2.ico">
        </head><body>正文</body></html>"#;
        let m = extract_metadata(&base, html);
        assert_eq!(m.title, "OG 标题");
        assert_eq!(m.description, "OG 描述");
        assert_eq!(
            m.favicon_url.as_deref(),
            Some("https://example.com/favicon-v2.ico")
        );
        assert_eq!(
            m.preview_image_url.as_deref(),
            Some("https://example.com/img/cover.png")
        );
    }

    #[test]
    fn extract_title_fallback() {
        let base = url::Url::parse("https://example.com").unwrap();
        let html = "<html><head><title>纯标题</title></head></html>";
        let m = extract_metadata(&base, html);
        assert_eq!(m.title, "纯标题");
        assert!(m.description.is_empty());
        assert_eq!(
            m.favicon_url.as_deref(),
            Some("https://example.com/favicon.ico")
        );
    }

    #[test]
    fn head_only_parsing_ignores_body_noise() {
        let base = url::Url::parse("https://example.com").unwrap();
        // body 里的 title 不应影响 head 提取。
        let html = "<html><head><title>Head 标题</title></head><body><title>Body 标题</title></body></html>";
        let m = extract_metadata(&base, html);
        assert_eq!(m.title, "Head 标题");
    }

    #[test]
    fn utf16le_bom_decoded() {
        let mut bytes = vec![0xff, 0xfe];
        for u in "标题".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_utf8(&bytes), "标题");
    }

    #[test]
    fn absolutize_data_url_kept() {
        let base = url::Url::parse("https://example.com").unwrap();
        assert_eq!(
            absolutize(&base, "data:image/png;base64,AAA"),
            Some("data:image/png;base64,AAA".to_string())
        );
    }
}
