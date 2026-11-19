# 配置参考

优先级：**环境变量 > `config.toml` > 默认值**。生成配置文件用
`linklist init`（完整字段与注释见生成的 `config.toml` 与
`config.toml.example`）。

## 全量字段

| 配置项 | 环境变量 | 默认值 | 说明 |
|---|---|---|---|
| database_url | `LINKLIST_DATABASE_URL`（兼容 `DATABASE_URL`，前者优先） | 空 | 数据库连接串；前缀决定后端，见[数据库](03-database.md) |
| listen | `LINKLIST_LISTEN` | `0.0.0.0:9090` | HTTP 监听地址 |
| data_dir | `LINKLIST_DATA_DIR` | `./data` | SQLite 数据库与快照/资产存储目录 |
| log | `LINKLIST_LOG` | `info` | 日志级别（RUST_LOG 语法） |
| session_ttl_secs | `LINKLIST_SESSION_TTL` | `604800` | 会话不活动过期（秒） |
| token_ttl_days | `LINKLIST_TOKEN_TTL` | `0` | API Token 有效期（天，0 = 永久） |
| admin_password | `LINKLIST_ADMIN_PASSWORD` | 无 | 预置管理员口令（仅库中无用户时生效；Docker 首启与 `init` 共用） |
| guest_sharing | `LINKLIST_GUEST_SHARING` | `false` | 匿名访客可见公开共享书签 |
| login_max_failures | `LINKLIST_LOGIN_MAX_FAILURES` | `10` | 登录失败限速阈值 |
| login_window_secs | `LINKLIST_LOGIN_WINDOW` | `300` | 限速窗口（秒） |
| fetch_timeout_secs | `LINKLIST_FETCH_TIMEOUT` | `30` | 抓取超时（秒） |
| fetch_max_bytes | `LINKLIST_FETCH_MAX_BYTES` | `5242880` | 抓取响应大小上限（字节） |
| fetch_max_redirects | `LINKLIST_FETCH_MAX_REDIRECTS` | `5` | 抓取重定向上限 |
| allowed_internal_hosts | `LINKLIST_ALLOWED_INTERNAL_HOSTS` | `[]` | SSRF 防护下的内网主机白名单（逗号分隔） |
| enable_oidc | `LINKLIST_ENABLE_OIDC` | `false` | 启用 OIDC 单点登录 |
| oidc_issuer | `LINKLIST_OIDC_ISSUER` | 无 | IdP 的 Issuer 地址；自动发现各 OIDC 端点（显式端点配置优先） |
| oidc_* | `LINKLIST_OIDC_*` | 无 | OIDC 端点与客户端配置（见 config.toml.example） |
| enable_auth_proxy | `LINKLIST_ENABLE_AUTH_PROXY` | `false` | 反向代理认证模式 |
| cors_allowed_origins | `LINKLIST_CORS_ALLOWED_ORIGINS` | `[]` | CORS 允许来源 |
| disable_url_validation | `LINKLIST_DISABLE_URL_VALIDATION` | `false` | 禁用书签 URL 格式校验 |
| enable_snapshots | `LINKLIST_ENABLE_SNAPSHOTS` | `false` | 启用 HTML 快照功能 |
| disable_asset_upload | `LINKLIST_DISABLE_ASSET_UPLOAD` | `false` | 禁用资产/快照上传 |
| disable_login_form | `LINKLIST_DISABLE_LOGIN_FORM` | `false` | 禁用密码登录表单（纯 SSO 部署） |
| disable_background_tasks | `LINKLIST_DISABLE_BACKGROUND_TASKS` | `false` | 禁用后台任务（metadata/snapshot/archive_lookup/preview） |

## 数据目录

`data_dir` 存放：

- SQLite 模式下的数据库文件 `linklist.db`（含 `-wal`/`-shm`）；
- 网页快照、预览图、favicon 缓存、上传资产等文件。

无论使用哪种数据库，快照与资产文件都写在本地数据目录——外部数据库
（PostgreSQL/MySQL）部署时该目录仍需持久化。

## 配置生效顺序示例

```bash
# config.toml 设了 database_url，但环境变量优先级更高
export LINKLIST_DATABASE_URL="postgres://user:pass@db:5432/linklist"
linklist run
```
