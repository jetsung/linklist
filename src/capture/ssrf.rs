use std::net::IpAddr;
use std::sync::OnceLock;

/// 允许抓取的内网主机白名单（对齐上游 LD_ALLOWED_INTERNAL_HOSTS，环境变量
/// `LINKLIST_ALLOWED_INTERNAL_HOSTS`，逗号分隔）。进程启动时注入一次，
/// 运行时输入无法在声明期求值，故用 OnceLock。
static ALLOWED_INTERNAL_HOSTS: OnceLock<Vec<String>> = OnceLock::new();

/// 注入内网主机白名单（应用启动时调用一次）。
pub fn set_allowed_internal_hosts(hosts: Vec<String>) {
    let _ = ALLOWED_INTERNAL_HOSTS.set(hosts);
}

/// 目标主机是否在白名单内（精确匹配，忽略大小写；支持 host 与 [v6] 形式）。
pub fn host_allowed(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    ALLOWED_INTERNAL_HOSTS
        .get()
        .is_some_and(|list| list.iter().any(|h| h.eq_ignore_ascii_case(bare)))
}

/// 校验出站抓取 URL：协议白名单 + 目标 IP 安全（拒绝私网/环回/链路本地等）。
pub fn validate_outbound_url(raw: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(raw).map_err(|e| format!("URL 解析失败: {e}"))?;
    // 协议白名单。
    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("不允许的协议: {other}")),
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL 缺少主机名".to_string())?;
    // 白名单主机跳过全部私网校验（IP 字面量与 localhost 都放行）。
    if host_allowed(host) {
        return Ok(parsed);
    }
    // 主机名是 IP 字面量时直接校验；域名交给 DNS 解析阶段（resolve_and_check）。
    let host_bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host_bare.parse::<IpAddr>() {
        check_ip(&ip)?;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return Err("禁止抓取 localhost".into());
    }
    Ok(parsed)
}

/// DNS 解析后校验所有解析出的 IP。
pub fn check_resolved_ips(ips: &[IpAddr]) -> Result<(), String> {
    if ips.is_empty() {
        return Err("DNS 解析无结果".into());
    }
    for ip in ips {
        check_ip(ip)?;
    }
    Ok(())
}

/// 单个 IP 安全检查：拒绝环回/私网/链路本地/未指定/组播/保留段。
pub fn check_ip(ip: &IpAddr) -> Result<(), String> {
    let bad = match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                // 保留段：0.0.0.0/8、100.64.0.0/10（CGNAT）、192.0.0.0/24、192.0.2.0/24、198.18.0.0/15、198.51.100.0/24、203.0.113.0/24、240.0.0.0/4
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                || o[0] >= 240
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // IPv4 映射地址按 IPv4 规则再查一遍。
                || v6.to_ipv4_mapped().map(|v4| check_ip(&IpAddr::V4(v4)).is_err()).unwrap_or(false)
                // 唯一本地 fc00::/7。
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                // 链路本地 fe80::/10。
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    };
    if bad {
        Err(format!("禁止抓取私有/保留地址: {ip}"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn dangerous_ip_matrix_rejected() {
        let cases: Vec<(IpAddr, bool)> = vec![
            (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), false), // 环回
            (IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), false),  // 私网
            (IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)), false), // 私网
            (IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), false), // 私网
            (IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)), false), // 链路本地
            (IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), false),   // 未指定
            (IpAddr::V4(Ipv4Addr::new(100, 64, 1, 1)), false), // CGNAT
            (IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), false), // TEST-NET
            (IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1)), false), // 基准测试
            (IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1)), false), // 保留
            (IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), true),    // 公网 OK
            (IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), true),    // 公网 OK
            (IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), false), // ::1
            (
                IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0xc0a8, 0x0101)),
                false,
            ), // ::ffff:192.168.1.1
            (
                IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0x6810, 0x85e5)),
                true,
            ), // 公网 OK
        ];
        for (ip, ok) in cases {
            assert_eq!(check_ip(&ip).is_ok(), ok, "IP {ip} 期望允许={ok}");
        }
    }

    #[test]
    fn url_scheme_whitelist() {
        assert!(validate_outbound_url("https://example.com/x").is_ok());
        assert!(validate_outbound_url("http://example.com").is_ok());
        assert!(validate_outbound_url("ftp://example.com").is_err());
        assert!(validate_outbound_url("file:///etc/passwd").is_err());
        assert!(validate_outbound_url("gopher://x").is_err());
    }

    #[test]
    fn url_ip_literal_checked() {
        assert!(validate_outbound_url("http://127.0.0.1/x").is_err());
        assert!(validate_outbound_url("http://[::1]/").is_err());
        assert!(validate_outbound_url("http://192.168.1.1/").is_err());
        assert!(validate_outbound_url("http://8.8.8.8/").is_ok());
        assert!(validate_outbound_url("http://localhost/").is_err());
    }

    #[test]
    fn url_parse_garbage() {
        assert!(validate_outbound_url("not a url").is_err());
        assert!(validate_outbound_url("").is_err());
    }

    #[test]
    fn allowed_internal_hosts_bypass() {
        // 注入白名单后：列出的内网 IP 字面量与内网域名放行（大小写不敏感）；
        // 未列入的仍拒绝。主机取值与其它测试不重叠，避免进程级白名单互相污染。
        set_allowed_internal_hosts(vec!["172.31.254.254".into(), "nas.local".into()]);
        assert!(validate_outbound_url("http://172.31.254.254/x").is_ok());
        assert!(validate_outbound_url("http://NAS.LOCAL/x").is_ok());
        assert!(validate_outbound_url("http://nas.local/").is_ok());
        assert!(validate_outbound_url("http://127.0.0.1/x").is_err());
        assert!(validate_outbound_url("http://10.0.0.5/").is_err());
        assert!(validate_outbound_url("http://localhost/").is_err());
    }
}
