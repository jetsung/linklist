# linklist

用 Rust 重写的自托管书签管理服务（替代 linkding），单二进制 + SQLite，无 Python/Node 运行时依赖。

## 功能

- 书签 CRUD、全文搜索（FTS5）、表达式搜索（`tag:rust`、`is:unread` 等）、标签管理
- 自动抓取网页元数据（标题/描述）、Archive.today 快照、web archive 查询
- 页面快照与资产上传（multipart，类型白名单 + 大小限制）
- 批量操作（加/删标签、已读/未读、删除）、归档与隔离视图
- Netscape 格式导入/导出、RSS 订阅源
- 多用户：注册（首个用户自动管理员）、会话 + API Token（`Authorization: Token lk_...`）
- 共享书签：登录用户可见的共享视图 + 可关闭的匿名公开共享（`guest_sharing`）
- 管理端：用户管理、原始数据、任务队列、全局设置、审计日志
- 前端：无框架静态多页（login/bookmarks/detail/shared/settings/admin），rust-embed 嵌入单二进制

## 快速开始

```bash
# 1. 构建（需要 Rust 1.8x+ 与 Node/pnpm 仅用于前端构建，产物已可预先嵌入）
pnpm build          # 生成 frontend/dist（rust-embed 构建期嵌入）
cargo build --release

# 2. 生成配置（可选；不生成则用默认值 + 环境变量）
./target/release/linklist init

# 3. 启动（默认 0.0.0.0:9090，数据目录 ./data）
./target/release/linklist run

# 4. 打开 http://localhost:9090/login 注册首个用户（自动成为管理员）
```

重置口令：

```bash
./target/release/linklist reset-password <username>
```

## 配置

优先级：环境变量 > `config.toml` > 默认值。

| 配置项 | 环境变量 | 默认值 | 说明 |
|---|---|---|---|
| listen | `LINKLIST_LISTEN` | `0.0.0.0:9090` | HTTP 监听地址 |
| data_dir | `LINKLIST_DATA_DIR` | `./data` | SQLite 与快照/资产存储目录 |
| log | `LINKLIST_LOG` | `info` | 日志级别（RUST_LOG 语法） |
| session_ttl_secs | `LINKLIST_SESSION_TTL` | `604800` | 会话不活动过期（秒） |
| token_ttl_days | `LINKLIST_TOKEN_TTL` | `0` | API Token 有效期（天，0 = 永久） |
| guest_sharing | `LINKLIST_GUEST_SHARING` | `false` | 匿名访客可见公开共享书签 |
| login_max_failures | `LINKLIST_LOGIN_MAX_FAILURES` | `10` | 登录失败限速阈值 |
| login_window_secs | `LINKLIST_LOGIN_WINDOW` | `300` | 限速窗口（秒） |
| fetch_timeout_secs | `LINKLIST_FETCH_TIMEOUT` | `30` | 抓取超时 |
| fetch_max_bytes | `LINKLIST_FETCH_MAX_BYTES` | `5242880` | 抓取响应大小上限 |
| fetch_max_redirects | `LINKLIST_FETCH_MAX_REDIRECTS` | `5` | 抓取重定向上限 |
| allowed_internal_hosts | `LINKLIST_ALLOWED_INTERNAL_HOSTS` | `[]` | SSRF 白名单（内网主机） |

## API 概览

基础路径 `/api/v1`，会话 Cookie 或 `Authorization: Token <token>` 认证。

| 分组 | 端点 |
|---|---|
| 认证 | `POST /auth/setup` `POST /auth/login` `POST /auth/logout` `GET /auth/me` `GET /auth/status` `POST /auth/password`；Token：`GET/POST /auth/tokens` `DELETE /auth/tokens/{id}` |
| 书签 | `GET/POST /bookmarks` `GET/PATCH/DELETE /bookmarks/{id}` `PUT /bookmarks/{id}/tags` `GET /bookmarks/tags` `POST /bookmarks/{id}/archive|unarchive` |
| 批量 | `POST /bookmarks/bulk/tags/add|tags/remove|mark-read|mark-unread|delete` |
| 抓取 | `POST /bookmarks/{id}/capture`（body `{"kind":"metadata|snapshot|archive_lookup"}`） `GET /bookmarks/{id}/capture-tasks` `POST /capture-tasks/{task_id}/retry` `GET /bookmarks/{id}/snapshot` |
| 资产 | `GET/POST /bookmarks/{id}/assets` `GET/DELETE /bookmarks/{id}/assets/{asset_id}` |
| 导入导出 | `POST /bookmarks/import`（Netscape） `GET /bookmarks/export` |
| 订阅 | `GET /feed/public_shared[?key=...]` |
| 偏好 | `GET/PUT /profile` `PUT /profile/auto-tagging-rules` `GET /profile/feed-key` |
| 共享 | `GET /shared`（登录） `GET /shared/public`（匿名受 `guest_sharing` 控制） |
| 管理 | `POST/GET /admin/users` `POST /admin/users/{id}/deactivate|activate|reset-password` `GET /admin/data/bookmarks` `GET /admin/tasks` `GET/PUT /admin/settings` `GET /admin/audit-logs` |

完整行为规格见 `openspec/specs/`。

## Docker 部署

```bash
# 标准镜像（多阶段构建：Node 构建前端 + Rust 构建后端）
docker build -f docker/Dockerfile -t linklist .

# 国内网络镜像（走代理拉取基础镜像）
docker build -f docker/CN.Dockerfile -t linklist .

# 运行
docker run -d --name linklist -p 9090:9090 -v linklist-data:/data linklist
# 或
docker compose -f docker/compose.yaml up -d
```

容器内数据目录为 `/data`，可通过环境变量覆盖任意配置项（如 `LINKLIST_GUEST_SHARING=true`）。

## 开发

```bash
pnpm build        # 前端产物（frontend/dist）
cargo test        # 131 个单元/集成测试
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# 端到端冒烟（release 构建 + 临时数据目录 + 32 项断言，自动清理）
bash scripts/smoke.sh
```
