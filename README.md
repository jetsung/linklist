# LinkList

用 Rust 重写的自托管书签管理服务（替代 linkding），单二进制交付，无 Python/Node 运行时依赖；支持 **SQLite（默认）/ PostgreSQL / MySQL** 三种数据库，连接串（`DATABASE_URL`）选择后端。完整文档见 `docs/`（`just docs-serve` 本地预览）。

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
# 1. 构建（需要 Rust 1.8x+；Node/pnpm 仅用于前端构建，产物已可预先嵌入）
pnpm -C frontend/astro build   # 生成 frontend/dist（rust-embed 构建期嵌入）
cargo build --release

# 2. 初始化项目（生成配置 + 数据目录 + 建库迁移；--admin-password 预置管理员 admin）
./target/release/linklist init

# 3. 启动（默认 0.0.0.0:9090，数据目录 ./data）
./target/release/linklist run

# 4. 打开 http://localhost:9090/login 注册首个用户（自动成为管理员）
```

常用命令（完整说明 `linklist help`）：

```bash
./target/release/linklist init --admin-password 'password123'   # 初始化并预置管理员
./target/release/linklist reset-password <username>             # 交互式重置口令
./target/release/linklist version                               # 版本号
```

## 数据库

连接通过单一连接串配置：环境变量 `LINKLIST_DATABASE_URL`（兼容通用别名 `DATABASE_URL`，前者优先）或 `config.toml` 的 `database_url`；**连接串前缀决定后端**（留空 = SQLite）：

```bash
export LINKLIST_DATABASE_URL="postgres://user:pass@host:5432/linklist"   # PostgreSQL
export LINKLIST_DATABASE_URL="mysql://user:pass@host:3306/linklist?characterEncoding=utf8mb4"  # MySQL
```

启动或 `init` 时自动执行迁移；迁移同时提供三种方言（`migrations/`、`migrations/postgres/`、`migrations/mysql/`），业务行为在三后端下等价。详见 `docs/03-database.md`。

## 配置

优先级：环境变量 > `config.toml` > 默认值。完整配置表见 `docs/02-configuration.md`。

| 配置项 | 环境变量 | 默认值 | 说明 |
|---|---|---|---|
| database_url | `LINKLIST_DATABASE_URL` / `DATABASE_URL` | 空 | 数据库连接串（前缀选后端） |
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
| 书签 | `GET/POST /bookmarks` `GET/PATCH/DELETE /bookmarks/{id}` `PUT /bookmarks/{id}/tags` `GET /bookmarks/tags` `POST /bookmarks/{id}/archive\|unarchive` |
| 批量 | `POST /bookmarks/bulk/tags/add\|tags/remove\|mark-read\|mark-unread\|delete` |
| 抓取 | `POST /bookmarks/{id}/capture`（body `{"kind":"metadata\|snapshot\|archive_lookup"}`） `GET /bookmarks/{id}/capture-tasks` `POST /capture-tasks/{task_id}/retry` `GET /bookmarks/{id}/snapshot` |
| 资产 | `GET/POST /bookmarks/{id}/assets` `GET/DELETE /bookmarks/{id}/assets/{asset_id}` |
| 导入导出 | `POST /bookmarks/import`（Netscape） `GET /bookmarks/export` |
| 订阅 | `GET /feed/public_shared[?key=...]` |
| 偏好 | `GET/PUT /profile` `PUT /profile/auto-tagging-rules` `GET /profile/feed-key` |
| 共享 | `GET /shared`（登录） `GET /shared/public`（匿名受 `guest_sharing` 控制） |
| 管理 | `POST/GET /admin/users` `POST /admin/users/{id}/deactivate\|activate\|reset-password` `GET /admin/data/bookmarks` `GET /admin/tasks` `GET/PUT /admin/settings` `GET /admin/audit-logs` |

## Docker 部署

```bash
# 标准镜像（多阶段构建：Node 构建前端 + Rust 构建后端）
docker build -f docker/Dockerfile -t linklist .

# 国内网络镜像（走代理拉取基础镜像）
docker build -f docker/CN.Dockerfile -t linklist .

# 运行
docker run -d --name linklist -p 9090:9090 -v linklist-data:/data linklist
# 或（SQLite 默认形态；just docker-up / docker-up-postgres / docker-up-mysql）
docker compose -f docker/compose.yaml up -d

# 外部数据库形态（官方镜像 + 健康检查 + 持久化卷，凭据写 docker/.env）
docker compose -f docker/compose.yaml -f docker/compose.postgres.yaml up -d
docker compose -f docker/compose.yaml -f docker/compose.mysql.yaml up -d
```

详见 `docs/04-docker.md`（三形态、凭据、数据卷、备份）。

## 开发

```bash
pnpm -C frontend/astro build   # 前端产物（frontend/dist）；或 just frontend-build
cargo test        # 单元/集成测试（功能实现配套单测并全量通过是提交前提）
cargo clippy --all-targets -- -D warnings
cargo fmt --check
just check        # 与 CI 参数逐字对齐：fmt + clippy + test

# 端到端冒烟（release 构建 + 临时数据目录 + 全链路断言，自动清理）
just smoke

# 文档站点（zensical，源 docs/）
just docs-build   # 构建到 site/
just docs-serve   # 本地预览 http://127.0.0.1:8000
```
