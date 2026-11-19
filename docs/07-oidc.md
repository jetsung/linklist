# OIDC 单点登录

LinkList 支持 OIDC（OpenID Connect）单点登录：采用**授权码流程 + PKCE
（S256）**，登录页展示"OIDC 登录"入口，回调后校验 ID Token 并按 claim
**自动创建账号并登录**（无需手动开户）。

## 前置条件

- 您的 IdP（Keycloak / Authentik / Casdoor / Auth0 / Logto 等）支持
  **授权码流程 + PKCE**；
- 在 IdP 上注册一个客户端，回调地址填：
  `http://<服务地址>/oidc/callback`（域名/端口按实际部署填写，路径固定
  为 `/oidc/callback`）。

## 配置项

全部通过环境变量配置（也可写入 `config.toml`，环境变量优先）：

| 环境变量 | 必填 | 默认值 | 说明 |
|---|---|---|---|
| `LINKLIST_ENABLE_OIDC` | 是 | `false` | 启用 OIDC 登录（启用后登录页显示 SSO 按钮） |
| `LINKLIST_OIDC_ISSUER` | 二选一 | 无 | IdP 的 Issuer 地址；配置后从 `{issuer}/.well-known/openid-configuration` 自动发现各端点（显式端点配置优先） |
| `LINKLIST_OIDC_AUTHORIZATION_ENDPOINT` | 二选一 | 无 | IdP 的授权端点（完整 URL）；与 Issuer 二选一，也可与 Issuer 混用（显式优先） |
| `LINKLIST_OIDC_TOKEN_ENDPOINT` | 二选一 | 无 | IdP 的 token 端点（完整 URL） |
| `LINKLIST_OIDC_CLIENT_ID` | 是 | 无 | 客户端 ID |
| `LINKLIST_OIDC_CLIENT_SECRET` | 视算法 | 无 | 客户端密钥（HS256 必填；RS256 建议也配） |
| `LINKLIST_OIDC_JWKS_ENDPOINT` | RS256 | 无 | IdP 的 JWKS 端点（RS256 验签用）；Issuer 方式下可省略 |
| `LINKLIST_OIDC_SIGN_ALGO` | 否 | `RS256` | ID Token 签名算法：`RS256`（走 JWKS）或 `HS256`（走 client_secret） |
| `LINKLIST_OIDC_USERNAME_CLAIM` | 否 | `email` | 从 claims 提取用户名的字段（如 `preferred_username`、`sub`） |
| `LINKLIST_OIDC_USER_ENDPOINT` | 否 | 无 | userinfo 端点（ID Token 缺用户名 claim 时兜底）；Issuer 方式下可省略 |

## 接入步骤

### 方式一：Issuer 自动发现（推荐）

标准 IdP 均提供发现文档，只需配置 Issuer 地址，各端点自动获取（进程内缓存，
不会每次登录重复请求）：

```bash
export LINKLIST_ENABLE_OIDC=true
export LINKLIST_OIDC_ISSUER="https://idp.example.com/realms/master"
export LINKLIST_OIDC_CLIENT_ID="linklist"
export LINKLIST_OIDC_CLIENT_SECRET="your-client-secret"
```

### 方式二：手动配置各端点

IdP 不提供发现文档、或需要覆盖某个端点时，逐项配置（见上方配置表）。

### 通用步骤

1. **在 IdP 注册应用**：类型选"机密客户端"或"公共客户端 + PKCE"，重定向
   URI 填 `http://<服务地址>/oidc/callback`。
2. **配置环境变量**（Issuer 方式见上；手动方式从 IdP 的发现文档
   `/.well-known/openid-configuration` 抄下各端点）。
3. **重启服务**，打开登录页 `http://<服务地址>/login`，应出现"OIDC 登录"
   按钮；点击后跳转 IdP 授权页。
4. **授权登录**：IdP 认证后回跳 `/oidc/callback`，LinkList 校验通过即自动
   建号并登录，跳转首页。

## 配置示例

手动配置端点的等价示例（与上面的 Issuer 方式二选一，也可混用——显式端点优先）：

```bash
export LINKLIST_ENABLE_OIDC=true
export LINKLIST_OIDC_AUTHORIZATION_ENDPOINT="https://idp.example.com/realms/master/protocol/openid-connect/auth"
export LINKLIST_OIDC_TOKEN_ENDPOINT="https://idp.example.com/realms/master/protocol/openid-connect/token"
export LINKLIST_OIDC_JWKS_ENDPOINT="https://idp.example.com/realms/master/protocol/openid-connect/certs"
export LINKLIST_OIDC_USER_ENDPOINT="https://idp.example.com/realms/master/protocol/openid-connect/userinfo"
export LINKLIST_OIDC_CLIENT_ID="linklist"
export LINKLIST_OIDC_CLIENT_SECRET="your-client-secret"
# 可选：RS256（默认）时无需再配；HS256 的 IdP 用：
# export LINKLIST_OIDC_SIGN_ALGO=HS256
# 可选：用户名字段（默认 email；也可用 preferred_username 等）
# export LINKLIST_OIDC_USERNAME_CLAIM=preferred_username
```

写入 `docker/.env` 或 systemd 环境文件后重启服务即可。

## 账号与权限行为

- 回调校验通过后按 `username_claim` 提取用户名，**库中不存在则自动创建**
  普通用户（非管理员），随即建立会话登录，并记录 `login`（来源 `oidc`）
  审计日志。
- 已存在的同名用户直接登录；**已被停用的用户返回 403**。
- OIDC 用户不使用密码登录（创建时写入随机占位口令）。
- **首个管理员仍建议通过 `init --admin-password` 或首次 setup 预置**；
  OIDC 自动建号不产生管理员。

## 纯 SSO 部署（关闭密码登录）

配合 `LINKLIST_DISABLE_LOGIN_FORM=true` 可隐藏登录页的密码表单，仅保留
SSO 入口，实现"只允许 IdP 登录"的部署形态：

```bash
export LINKLIST_ENABLE_OIDC=true
export LINKLIST_DISABLE_LOGIN_FORM=true
# …其余 OIDC 端点与客户端配置同上
```

## 反向代理部署注意

回调地址按请求的 `Host` 头推导（`http(s)://<Host>/oidc/callback`）。若置于
Nginx/Caddy 等反代之后，请确保反代正确转发 `Host` 与协议头（如
`X-Forwarded-Proto: https`），否则推导出的回调地址与 IdP 注册值不一致会导致
授权失败。

## 常见问题排查

| 现象 | 可能原因 |
|---|---|
| IdP 报 `redirect_uri` 不匹配 | 回调地址与注册值不一致（协议/域名/端口/路径），或反代未正确转发 Host |
| 回调报 state 无效 | 授权跳转与会话间隔过久（state 有短时效），或登录会话丢失；重新从登录页进入 |
| 日志出现 JWKS/验签失败 | `LINKLIST_OIDC_JWKS_ENDPOINT` 不可达、算法配置不符（RS256/HS256 混用）、或 client_secret 错误 |
| 提示"缺少用户名 claim" | ID Token 与 userinfo 中均无所配字段；改 `LINKLIST_OIDC_USERNAME_CLAIM`（如 `preferred_username`）或补配 userinfo 端点 |
| 登录页没有 SSO 按钮 | `LINKLIST_ENABLE_OIDC` 未设为 `true` 或服务未重启生效 |
| OIDC 用户登录后无法管理 | 自动建号均为普通用户；管理员需通过 `init --admin-password`、首次 setup 或管理端创建 |
