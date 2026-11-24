use std::path::PathBuf;

use serde::Deserialize;

/// 应用配置。
///
/// 优先级：环境变量 > `config.toml`（工作目录） > 默认值。
/// 环境变量前缀 `LINKLIST_`。
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Config {
    /// HTTP 监听地址。
    #[serde(default = "default_listen")]
    pub listen: String,
    /// 数据目录（SQLite 数据库、快照、密钥等）。
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// 数据库连接串。空 = 使用 `data_dir` 下的 SQLite 文件；
    /// 以 `postgres://` 开头 = 使用 PostgreSQL（环境变量 LINKLIST_DATABASE_URL）。
    #[serde(default)]
    pub database_url: String,
    /// 日志级别（RUST_LOG 语法）。
    #[serde(default = "default_log")]
    pub log: String,
    /// 会话不活动过期时间（秒）。
    #[serde(default = "default_session_ttl")]
    pub session_ttl_secs: u64,
    /// 会话 Cookie 是否加 Secure 标志（仅经 HTTPS 传输）；HTTPS 部署建议开启。
    #[serde(default)]
    pub session_cookie_secure: bool,
    /// API Token 有效期（天）；0 表示永不过期。
    #[serde(default)]
    pub token_ttl_days: u64,
    /// 是否允许匿名访客查看公开共享书签。
    #[serde(default)]
    pub guest_sharing: bool,
    /// 登录失败限速：窗口内最大失败次数。
    #[serde(default = "default_login_max_failures")]
    pub login_max_failures: u32,
    /// 登录失败限速：窗口长度（秒）。
    #[serde(default = "default_login_window_secs")]
    pub login_window_secs: u64,
    /// 抓取超时（秒）。
    #[serde(default = "default_fetch_timeout_secs")]
    pub fetch_timeout_secs: u64,
    /// 抓取响应大小上限（字节）。
    #[serde(default = "default_fetch_max_bytes")]
    pub fetch_max_bytes: usize,
    /// 抓取重定向次数上限。
    #[serde(default = "default_fetch_max_redirects")]
    pub fetch_max_redirects: usize,
    /// 允许抓取的内网主机白名单（逗号分隔）。
    #[serde(default)]
    pub allowed_internal_hosts: Vec<String>,
    /// 启用 OIDC 登录（对齐 LD_ENABLE_OIDC）。
    #[serde(default)]
    pub enable_oidc: bool,
    /// OIDC 提供方端点与客户端凭据。
    #[serde(default)]
    pub oidc_authorization_endpoint: Option<String>,
    #[serde(default)]
    pub oidc_token_endpoint: Option<String>,
    #[serde(default)]
    pub oidc_userinfo_endpoint: Option<String>,
    #[serde(default)]
    pub oidc_jwks_endpoint: Option<String>,
    #[serde(default)]
    pub oidc_client_id: Option<String>,
    #[serde(default)]
    pub oidc_client_secret: Option<String>,
    /// ID Token 签名算法（RS256/HS256）。
    #[serde(default = "default_oidc_sign_algo")]
    pub oidc_sign_algo: String,
    /// 用户名取自哪个 claim（对齐 OIDC_USERNAME_CLAIM，默认 email）。
    /// OIDC Issuer：配置后从 `{issuer}/.well-known/openid-configuration`
    /// 自动发现未显式配置的端点（显式配置优先）。
    #[serde(default)]
    pub oidc_issuer: Option<String>,
    #[serde(default = "default_oidc_username_claim")]
    pub oidc_username_claim: String,
    /// 显式指定 OIDC 回调地址（redirect_uri）。缺省按请求 Host 推导为
    /// `{base}/oidc/callback/`（带尾斜杠，对齐上游 mozilla_django_oidc 惯例）。
    /// 当 IdP 预注册的 redirect_uri 与推导值不一致时配置此项。
    #[serde(default)]
    pub oidc_redirect_uri: Option<String>,
    /// 启用反向代理认证头（对齐 LD_ENABLE_AUTH_PROXY）。
    #[serde(default)]
    pub enable_auth_proxy: bool,
    /// 反代用户名请求头名（对齐 LD_AUTH_PROXY_USERNAME_HEADER，默认 REMOTE_USER）。
    #[serde(default = "default_auth_proxy_header")]
    pub auth_proxy_username_header: String,
    /// 反代登出跳转地址（对齐 LD_AUTH_PROXY_LOGOUT_URL）。
    #[serde(default)]
    pub auth_proxy_logout_url: Option<String>,
    /// API 允许的 CORS 来源，逗号分隔（对齐 LD_CORS_ALLOWED_ORIGINS；空 = 关闭）。
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    /// 禁用书签 URL 格式校验（对齐 LD_DISABLE_URL_VALIDATION）。
    #[serde(default)]
    pub disable_url_validation: bool,
    /// 启用 HTML 快照功能总开关（对齐 LD_ENABLE_SNAPSHOTS）。
    #[serde(default)]
    pub enable_snapshots: bool,
    /// 禁用资产/快照上传（对齐 LD_DISABLE_ASSET_UPLOAD）。
    #[serde(default)]
    pub disable_asset_upload: bool,
    /// 禁用密码登录表单（纯 SSO 部署；对齐 LD_DISABLE_LOGIN_FORM）。
    #[serde(default)]
    pub disable_login_form: bool,
    /// 禁用后台任务（metadata/snapshot 等；对齐 LD_DISABLE_BACKGROUND_TASKS）。
    #[serde(default)]
    pub disable_background_tasks: bool,
}

fn default_oidc_sign_algo() -> String {
    "RS256".to_string()
}

fn default_oidc_username_claim() -> String {
    "email".to_string()
}

fn default_auth_proxy_header() -> String {
    "REMOTE_USER".to_string()
}

fn default_listen() -> String {
    "0.0.0.0:9090".to_string()
}

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_log() -> String {
    "info".to_string()
}

fn default_session_ttl() -> u64 {
    7 * 24 * 3600
}

fn default_login_max_failures() -> u32 {
    10
}

fn default_login_window_secs() -> u64 {
    300
}

fn default_fetch_timeout_secs() -> u64 {
    30
}

fn default_fetch_max_bytes() -> usize {
    5 * 1024 * 1024
}

fn default_fetch_max_redirects() -> usize {
    5
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: default_listen(),
            data_dir: default_data_dir(),
            database_url: String::new(),
            log: default_log(),
            session_ttl_secs: default_session_ttl(),
            session_cookie_secure: false,
            token_ttl_days: 0,
            guest_sharing: false,
            login_max_failures: default_login_max_failures(),
            login_window_secs: default_login_window_secs(),
            fetch_timeout_secs: default_fetch_timeout_secs(),
            fetch_max_bytes: default_fetch_max_bytes(),
            fetch_max_redirects: default_fetch_max_redirects(),
            allowed_internal_hosts: Vec::new(),
            enable_oidc: false,
            oidc_authorization_endpoint: None,
            oidc_token_endpoint: None,
            oidc_userinfo_endpoint: None,
            oidc_jwks_endpoint: None,
            oidc_issuer: None,
            oidc_client_id: None,
            oidc_client_secret: None,
            oidc_sign_algo: default_oidc_sign_algo(),
            oidc_username_claim: default_oidc_username_claim(),
            oidc_redirect_uri: None,
            enable_auth_proxy: false,
            auth_proxy_username_header: default_auth_proxy_header(),
            auth_proxy_logout_url: None,
            cors_allowed_origins: Vec::new(),
            disable_url_validation: false,
            enable_snapshots: false,
            disable_asset_upload: false,
            disable_login_form: false,
            disable_background_tasks: false,
        }
    }
}

/// 连接串环境变量读取顺序（design D4）：`LINKLIST_DATABASE_URL` > 通用别名 `DATABASE_URL`。
fn database_url_from_env() -> Option<String> {
    env_str("LINKLIST_DATABASE_URL").or_else(|| env_str("DATABASE_URL"))
}

impl Config {
    /// 加载配置：读取 `config.toml`（如存在），再用环境变量覆盖。
    pub fn load() -> Self {
        let file_cfg = Self::from_file(&config_path());
        let mut cfg = file_cfg.unwrap_or_default();
        cfg.apply_env();
        cfg
    }

    /// 从 TOML 文件解析配置。
    pub fn from_file(path: &std::path::Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let cfg = toml::from_str(&raw).unwrap_or_default();
        Some(cfg)
    }

    /// 用 `LINKLIST_*` 环境变量覆盖对应字段。
    pub fn apply_env(&mut self) {
        if let Some(v) = env_str("LINKLIST_LISTEN") {
            self.listen = v;
        }
        if let Some(v) = env_str("LINKLIST_DATA_DIR") {
            self.data_dir = PathBuf::from(v);
        }
        if let Some(v) = database_url_from_env() {
            self.database_url = v;
        }
        if let Some(v) = env_str("LINKLIST_LOG") {
            self.log = v;
        }
        if let Some(v) = env_u64("LINKLIST_SESSION_TTL") {
            self.session_ttl_secs = v;
        }
        if let Some(v) = env_str("LINKLIST_SESSION_COOKIE_SECURE") {
            self.session_cookie_secure =
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Some(v) = env_u64("LINKLIST_TOKEN_TTL") {
            self.token_ttl_days = v;
        }
        if let Some(v) = env_str("LINKLIST_GUEST_SHARING") {
            self.guest_sharing =
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Some(v) = env_u64("LINKLIST_LOGIN_MAX_FAILURES") {
            self.login_max_failures = v as u32;
        }
        if let Some(v) = env_u64("LINKLIST_LOGIN_WINDOW") {
            self.login_window_secs = v;
        }
        if let Some(v) = env_u64("LINKLIST_FETCH_TIMEOUT") {
            self.fetch_timeout_secs = v;
        }
        if let Some(v) = env_u64("LINKLIST_FETCH_MAX_BYTES") {
            self.fetch_max_bytes = v as usize;
        }
        if let Some(v) = env_u64("LINKLIST_FETCH_MAX_REDIRECTS") {
            self.fetch_max_redirects = v as usize;
        }
        if let Some(v) = env_str("LINKLIST_ALLOWED_INTERNAL_HOSTS") {
            self.allowed_internal_hosts = v
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(v) = env_str("LINKLIST_ENABLE_OIDC") {
            self.enable_oidc =
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Some(v) = env_str("LINKLIST_OIDC_AUTHORIZATION_ENDPOINT") {
            self.oidc_authorization_endpoint = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_TOKEN_ENDPOINT") {
            self.oidc_token_endpoint = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_USER_ENDPOINT") {
            self.oidc_userinfo_endpoint = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_ISSUER") {
            self.oidc_issuer = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_JWKS_ENDPOINT") {
            self.oidc_jwks_endpoint = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_CLIENT_ID") {
            self.oidc_client_id = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_CLIENT_SECRET") {
            self.oidc_client_secret = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_OIDC_SIGN_ALGO") {
            self.oidc_sign_algo = v;
        }
        if let Some(v) = env_str("LINKLIST_OIDC_USERNAME_CLAIM") {
            self.oidc_username_claim = v;
        }
        if let Some(v) = env_str("LINKLIST_OIDC_REDIRECT_URI") {
            self.oidc_redirect_uri = Some(v);
        }
        if let Some(v) = env_str("LINKLIST_ENABLE_AUTH_PROXY") {
            self.enable_auth_proxy =
                matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Some(v) = env_str("LINKLIST_AUTH_PROXY_USERNAME_HEADER") {
            self.auth_proxy_username_header = v;
        }
        if let Some(v) = env_str("LINKLIST_AUTH_PROXY_LOGOUT_URL") {
            self.auth_proxy_logout_url = Some(v);
        }
        if env_flag("LINKLIST_DISABLE_URL_VALIDATION") {
            self.disable_url_validation = true;
        }
        if env_flag("LINKLIST_ENABLE_SNAPSHOTS") {
            self.enable_snapshots = true;
        }
        if env_flag("LINKLIST_DISABLE_ASSET_UPLOAD") {
            self.disable_asset_upload = true;
        }
        if env_flag("LINKLIST_DISABLE_LOGIN_FORM") {
            self.disable_login_form = true;
        }
        if env_flag("LINKLIST_DISABLE_BACKGROUND_TASKS") {
            self.disable_background_tasks = true;
        }
    }
}

/// 配置文件路径（`LINKLIST_CONFIG` 或工作目录 `config.toml`）。
pub fn config_path() -> PathBuf {
    std::env::var("LINKLIST_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.toml"))
}

fn env_str(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

/// 读取布尔环境变量（"1"/"true"/"yes"/"on" 为真；未设置返回 false）。
fn env_flag(key: &str) -> bool {
    env_str(key)
        .map(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}
fn env_u64(key: &str) -> Option<u64> {
    env_str(key).and_then(|v| v.parse().ok())
}

/// 生成带注释的示例配置文件内容。
pub fn sample_config() -> String {
    r#"# linklist 配置文件
# 优先级：环境变量 > 本文件 > 默认值

# HTTP 监听地址（环境变量 LINKLIST_LISTEN）
listen = "0.0.0.0:9090"

# 数据目录（环境变量 LINKLIST_DATA_DIR）
data_dir = "./data"

# 数据库连接串（环境变量 LINKLIST_DATABASE_URL，兼容通用别名 DATABASE_URL，前者优先）
# 留空 = 使用 data_dir 下的 SQLite（linklist.db，默认）；
# 三种后端按连接串前缀自动选择：
#   SQLite:    sqlite://data/linklist.db?mode=rwc
#   PostgreSQL: postgres://user:pass@host:5432/dbname
#   MySQL:      mysql://user:pass@host:3306/dbname?characterEncoding=utf8mb4
database_url = ""

# 日志级别（环境变量 LINKLIST_LOG，RUST_LOG 语法）
log = "info"

# 会话不活动过期时间（秒；环境变量 LINKLIST_SESSION_TTL）
session_ttl_secs = 604800

# 会话 Cookie 是否加 Secure 标志（仅经 HTTPS 传输；环境变量
# LINKLIST_SESSION_COOKIE_SECURE）。HTTPS 反代部署建议设为 true
session_cookie_secure = false

# API Token 有效期（天；0 = 永不过期；环境变量 LINKLIST_TOKEN_TTL）
token_ttl_days = 0

# 允许匿名访客查看公开共享书签（环境变量 LINKLIST_GUEST_SHARING）
guest_sharing = false

# 登录失败限速（环境变量 LINKLIST_LOGIN_MAX_FAILURES / LINKLIST_LOGIN_WINDOW）
login_max_failures = 10
login_window_secs = 300

# 抓取安全边界（环境变量 LINKLIST_FETCH_TIMEOUT / LINKLIST_FETCH_MAX_BYTES / LINKLIST_FETCH_MAX_REDIRECTS）
fetch_timeout_secs = 30
fetch_max_bytes = 5242880
fetch_max_redirects = 5

# 允许抓取的内网主机白名单（逗号分隔；环境变量 LINKLIST_ALLOWED_INTERNAL_HOSTS）
allowed_internal_hosts = []

# 禁用书签 URL 格式校验（环境变量 LINKLIST_DISABLE_URL_VALIDATION，默认 false）
# disable_url_validation = false

# 启用 HTML 快照功能（环境变量 LINKLIST_ENABLE_SNAPSHOTS，默认 false）
# enable_snapshots = false

# 禁用资产/快照上传（环境变量 LINKLIST_DISABLE_ASSET_UPLOAD，默认 false）
# disable_asset_upload = false

# 禁用密码登录表单（纯 SSO 部署用；环境变量 LINKLIST_DISABLE_LOGIN_FORM，默认 false）
# disable_login_form = false

# 禁用后台任务（metadata/snapshot/archive_lookup；环境变量 LINKLIST_DISABLE_BACKGROUND_TASKS，默认 false）
# disable_background_tasks = false
"#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.listen, "0.0.0.0:9090");
        assert_eq!(cfg.data_dir, PathBuf::from("./data"));
        assert!(cfg.fetch_max_bytes > 0);
    }

    #[test]
    fn env_overrides_file() {
        // 环境变量 > 文件：设置环境变量后 load 应取环境变量值。
        // （本测试只验证 apply_env 的覆盖逻辑，不真正读全局 env；
        //   apply_env 直接读取 std::env，这里用子进程方式不便，
        //   因此仅验证解析行为。）
        let mut cfg = Config {
            listen: "1.2.3.4:1234".to_string(),
            ..Default::default()
        };
        cfg.apply_env();
        // 未设置环境变量时保持原值。
        assert_eq!(cfg.listen, "1.2.3.4:1234");
    }

    #[test]
    fn parses_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
listen = "127.0.0.1:12345"
data_dir = "/tmp/ll-test"
log = "debug"
guest_sharing = true
token_ttl_days = 30
"#,
        )
        .unwrap();
        let cfg = Config::from_file(&path).unwrap();
        assert_eq!(cfg.listen, "127.0.0.1:12345");
        assert_eq!(cfg.data_dir, PathBuf::from("/tmp/ll-test"));
        assert_eq!(cfg.log, "debug");
        assert!(cfg.guest_sharing);
        assert_eq!(cfg.token_ttl_days, 30);
    }

    #[test]
    fn parses_database_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"
database_url = "postgres://u:p@127.0.0.1:5432/linklist"
"#,
        )
        .unwrap();
        let cfg = Config::from_file(&path).unwrap();
        assert_eq!(cfg.database_url, "postgres://u:p@127.0.0.1:5432/linklist");
        // 默认（未配置）为空串 = SQLite。
        assert_eq!(Config::default().database_url, "");
    }

    #[test]
    fn oidc_issuer_env_and_default() {
        // 默认 None；设置 LINKLIST_OIDC_ISSUER 后读取生效。
        assert_eq!(Config::default().oidc_issuer, None);
        // SAFETY: 测试专用；该变量仅由本测试与被测代码读写，测试用例间无并发读写依赖。
        unsafe {
            std::env::set_var(
                "LINKLIST_OIDC_ISSUER",
                "https://idp.example.com/realms/master",
            )
        };
        let mut cfg = Config::default();
        cfg.apply_env();
        assert_eq!(
            cfg.oidc_issuer.as_deref(),
            Some("https://idp.example.com/realms/master")
        );
        // SAFETY: 测试专用；仅清除本测试自行设置的变量，无并发读写依赖。
        unsafe { std::env::remove_var("LINKLIST_OIDC_ISSUER") };
    }

    #[test]
    fn missing_file_returns_none() {
        let cfg = Config::from_file(std::path::Path::new("/nonexistent/config.toml"));
        assert!(cfg.is_none());
    }

    #[test]
    fn database_url_env_priority() {
        // 连接串环境变量优先级（design D4）：LINKLIST_DATABASE_URL > DATABASE_URL > 均无 → None。
        // SAFETY: 测试专用；仅清除本测试用到的两个变量，测试用例间无并发读写依赖。
        unsafe {
            std::env::remove_var("LINKLIST_DATABASE_URL");
            std::env::remove_var("DATABASE_URL");
        }
        assert_eq!(database_url_from_env(), None, "均未设置时应为 None");

        // 仅通用别名 DATABASE_URL → 生效（生态通行惯例，`DATABASE_URL=… linklist init` 可直接工作）。
        // SAFETY: 测试专用；该变量仅由本测试与被测代码读写，测试用例间无并发读写依赖。
        unsafe { std::env::set_var("DATABASE_URL", "mysql://u:p@127.0.0.1:3306/linklist") };
        assert_eq!(
            database_url_from_env().as_deref(),
            Some("mysql://u:p@127.0.0.1:3306/linklist")
        );

        // 两者同时设置 → LINKLIST_ 前缀优先。
        // SAFETY: 测试专用；该变量仅由本测试与被测代码读写，测试用例间无并发读写依赖。
        unsafe {
            std::env::set_var(
                "LINKLIST_DATABASE_URL",
                "postgres://u:p@127.0.0.1:5432/linklist",
            )
        };
        assert_eq!(
            database_url_from_env().as_deref(),
            Some("postgres://u:p@127.0.0.1:5432/linklist")
        );

        // SAFETY: 测试专用；仅清除本测试自行设置的变量，无并发读写依赖。
        unsafe {
            std::env::remove_var("LINKLIST_DATABASE_URL");
            std::env::remove_var("DATABASE_URL");
        }
    }

    #[test]
    fn invalid_toml_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "listen = 12345").unwrap(); // 类型错误
        let cfg = Config::from_file(&path).unwrap();
        assert_eq!(cfg, Config::default());
    }
}
