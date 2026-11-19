# REST API

基础路径 `/api/v1`，认证方式两种：

- **会话 Cookie**：登录后自动携带（Web 界面使用）。
- **API Token**：`Authorization: Token lk_...`（`GET /auth/tokens` 创建，
  `token_ttl_days` 控制有效期）。

统一错误结构：

```json
{ "error": { "code": "bad_request", "message": "..." } }
```

`code` 稳定可编程处理；内部错误不向客户端泄露细节（服务端日志可见）。

## 端点概览

| 分组 | 端点 |
|---|---|
| 认证 | `POST /auth/setup`（首个管理员） `POST /auth/login` `POST /auth/logout` `GET /auth/me` `GET /auth/status` `POST /auth/password`；Token：`GET/POST /auth/tokens` `DELETE /auth/tokens/{id}` |
| 书签 | `GET/POST /bookmarks` `GET/PATCH/DELETE /bookmarks/{id}` `PUT /bookmarks/{id}/tags` `GET /bookmarks/tags` `GET /bookmarks/tags/{name}` `POST /bookmarks/{id}/archive\|unarchive` |
| 批量 | `POST /bookmarks/bulk/tags/add\|tags/remove\|mark-read\|mark-unread\|delete` |
| 抓取 | `POST /bookmarks/{id}/capture`（body `{"kind":"metadata\|snapshot\|archive_lookup"}`） `GET /bookmarks/{id}/capture-tasks` `POST /capture-tasks/{task_id}/retry` `GET /bookmarks/{id}/snapshot` |
| 资产 | `GET/POST /bookmarks/{id}/assets` `GET/DELETE /bookmarks/{id}/assets/{asset_id}` |
| 导入导出 | `POST /bookmarks/import`（Netscape） `GET /bookmarks/export` |
| 订阅 | `GET /feed/public_shared[?key=...]` |
| 偏好 | `GET/PUT /profile` `PUT /profile/auto-tagging-rules` `GET/PUT /profile/search-preferences` `GET /profile/feed-key` |
| 共享 | `GET /shared`（登录） `GET /shared/tags` `GET /shared/users` `GET /shared/public`（匿名） `GET /shared/viewer-profile` |
| 管理 | `POST/GET /admin/users` `POST /admin/users/{id}/deactivate\|activate\|reset-password` `GET /admin/data/bookmarks` `GET /admin/tasks` `GET/PUT /admin/settings` `GET /admin/audit-logs` |

## 初始化流程（程序化）

```bash
# 1. 首次初始化管理员（仅当库中无用户；也可用 init --admin-password 预置）
curl -X POST http://localhost:9090/api/v1/auth/setup \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"password123"}'

# 2. 登录拿会话 Cookie，或创建 API Token
curl -X POST http://localhost:9090/api/v1/auth/tokens \
  -b cookies.txt -H "Content-Type: application/json" \
  -d '{"name":"ci"}'
```

## 搜索接口

`GET /api/v1/bookmarks?q=tag:rust+is:unread&sort=title_asc`——表达式语法见
[功能使用](06-features.md)。共享视图接口（`/shared*`）支持同样的
表达式与标签/用户筛选。
