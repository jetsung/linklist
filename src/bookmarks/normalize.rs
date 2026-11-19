use sha2::{Digest, Sha256};

/// URL 规范化 + sha256 哈希。
///
/// 规则（对齐 linkding `url_tagging`/`build_url_hash` 语义的简化版）：
/// - 去除首尾空白；
/// - 协议与主机转小写（保留路径大小写）；
/// - 默认端口省略（http:80 / https:443）；
/// - 去除 fragment（#）；
/// - 去除常见跟踪参数（utm_*、fbclid、gclid、ref、mc_cid、mc_eid 等）；
/// - 剩余查询参数按名称排序。
pub fn normalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    // 无协议时按 https 补全（linkding 对无 scheme 输入的宽容处理）。
    let has_scheme = trimmed.contains("://");
    let candidate = if has_scheme {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    let (scheme, rest) = match candidate.split_once("://") {
        Some((s, r)) => (s.to_ascii_lowercase(), r.to_string()),
        None => ("https".to_string(), candidate),
    };

    let (authority, path_and_query) = match rest.find(['/', '?', '#']) {
        Some(idx) => (rest[..idx].to_string(), rest[idx..].to_string()),
        None => (rest.clone(), String::new()),
    };

    // 去掉用户信息（user:pass@host）。
    let authority = match authority.rsplit_once('@') {
        Some((_userinfo, host)) => host.to_string(),
        None => authority,
    };

    // 拆 host:port。
    let (host, port) = split_host_port(&authority, &scheme);

    // 处理 path + query + fragment。
    let (path_query, _fragment) = match path_and_query.split_once('#') {
        Some((pq, _f)) => (pq.to_string(), Some(_f.to_string())),
        None => (path_and_query.clone(), None),
    };
    let (path, query) = match path_query.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (path_query, None),
    };

    // 过滤跟踪参数并排序。
    let filtered_query = query.map(|q| {
        let mut pairs: Vec<(String, String)> = q
            .split('&')
            .filter(|s| !s.is_empty())
            .filter_map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                let k = k.to_ascii_lowercase();
                if is_tracking_param(&k) {
                    None
                } else {
                    Some((k, v.to_string()))
                }
            })
            .collect();
        pairs.sort();
        pairs
            .iter()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.clone()
                } else {
                    format!("{k}={v}")
                }
            })
            .collect::<Vec<_>>()
            .join("&")
    });

    let mut out = format!("{scheme}://{host}");
    if let Some(p) = port {
        out.push(':');
        out.push_str(&p);
    }
    // 空路径 → 根路径；保留非根路径原样（去尾部多余斜杠仅对根）。
    if path.is_empty() || path == "/" {
        out.push('/');
    } else {
        out.push_str(&path);
    }
    if let Some(q) = filtered_query
        && !q.is_empty()
    {
        out.push('?');
        out.push_str(&q);
    }
    out
}

fn split_host_port(authority: &str, scheme: &str) -> (String, Option<String>) {
    // IPv6 字面量：[::1]:8080
    if authority.starts_with('[')
        && let Some(end) = authority.find(']')
    {
        let host = authority[..=end].to_string();
        let port = authority[end + 1..]
            .strip_prefix(':')
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());
        return (host, port);
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            let port = normalize_port(scheme, p);
            (h.to_ascii_lowercase(), port)
        }
        _ => (authority.to_ascii_lowercase(), None),
    }
}

fn normalize_port(scheme: &str, port: &str) -> Option<String> {
    match (scheme, port) {
        ("http", "80") | ("https", "443") => None,
        _ => Some(port.to_string()),
    }
}

fn is_tracking_param(key: &str) -> bool {
    const TRACKING: &[&str] = &[
        "fbclid", "gclid", "dclid", "msclkid", "mc_cid", "mc_eid", "ref", "ref_src", "ref_url",
        "igshid", "si",
    ];
    key.starts_with("utm_") || TRACKING.contains(&key)
}

/// URL 哈希（规范化后 sha256 hex）。
pub fn url_hash(normalized: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 书签 URL 校验（对齐上游 BookmarkURLValidator 的 strict 语义）：
/// - 必须能解析出 scheme 与 host；
/// - scheme 限 http/https（本系统抓取/favicon 均按 http(s) 处理）。
///
/// `LD_DISABLE_URL_VALIDATION` 对应的 `disable_url_validation=true` 时不调用。
pub fn validate_bookmark_url(raw: &str) -> Result<(), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("URL 不能为空".into());
    }
    let parsed = url::Url::parse(trimmed)
        .or_else(|_| url::Url::parse(&format!("https://{trimmed}")))
        .map_err(|_| "URL 格式无效".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!(
            "仅支持 http/https URL，收到 scheme: {}",
            parsed.scheme()
        ));
    }
    if parsed.host_str().unwrap_or("").is_empty() {
        return Err("URL 缺少 host".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercases_scheme_and_host() {
        assert_eq!(
            normalize_url("HTTPS://Example.COM/Path"),
            "https://example.com/Path"
        );
    }

    #[test]
    fn adds_missing_scheme() {
        assert_eq!(normalize_url("example.com"), "https://example.com/");
    }

    #[test]
    fn strips_fragment() {
        assert_eq!(
            normalize_url("https://a.com/page#section"),
            "https://a.com/page"
        );
    }

    #[test]
    fn strips_tracking_params() {
        let out = normalize_url("https://a.com/p?utm_source=x&id=5&fbclid=y");
        assert_eq!(out, "https://a.com/p?id=5");
    }

    #[test]
    fn sorts_remaining_params() {
        let out = normalize_url("https://a.com/p?b=2&a=1");
        assert_eq!(out, "https://a.com/p?a=1&b=2");
    }

    #[test]
    fn drops_default_ports() {
        assert_eq!(normalize_url("http://a.com:80/"), "http://a.com/");
        assert_eq!(normalize_url("https://a.com:443/"), "https://a.com/");
        assert_eq!(normalize_url("http://a.com:8080/"), "http://a.com:8080/");
    }

    #[test]
    fn strips_userinfo() {
        assert_eq!(normalize_url("https://user:pass@a.com/"), "https://a.com/");
    }

    #[test]
    fn hash_is_stable_and_differs() {
        let a = url_hash(&normalize_url("https://a.com/"));
        let b = url_hash(&normalize_url("https://A.com/#frag"));
        let c = url_hash(&normalize_url("https://b.com/"));
        assert_eq!(a, b, "规范化后相同 URL 哈希一致");
        assert_ne!(a, c);
    }

    #[test]
    fn validate_accepts_http_https_and_auto_scheme() {
        assert!(validate_bookmark_url("https://example.com/x").is_ok());
        assert!(validate_bookmark_url("http://example.com").is_ok());
        assert!(validate_bookmark_url("example.com/no-scheme").is_ok());
    }

    #[test]
    fn validate_rejects_bad_urls() {
        assert!(validate_bookmark_url("").is_err());
        assert!(validate_bookmark_url("   ").is_err());
        assert!(validate_bookmark_url("ftp://example.com/x").is_err());
        assert!(validate_bookmark_url("javascript:alert(1)").is_err());
    }
}
