# 本地开发

## 环境与构建

工具链（rust / node / pnpm / just 等）通过 mise 安装。注意：`cargo build`
会在编译期经 rust-embed 嵌入 `frontend/dist`，改完前端先 `just frontend-build`。

```bash
just frontend-install   # 安装前端依赖（frontend/astro/）
just frontend-build     # 构建前端产物到 frontend/dist
just backend            # 启动后端开发服务（http://127.0.0.1:9090）
just frontend           # 前端 dev server（http://localhost:4321）
just dev                # 同时启动后端与前端本地调试
just build              # 发布版构建（先前端产物再 cargo build --release）
```

## 架构

```
src/
  main.rs         CLI 入口（run/init/reset-password/version/help）与服务组装
  config.rs       配置分层：环境变量（LINKLIST_*）> config.toml > 默认值
  db.rs           跨后端数据库层：DbPool/DbRow/DbQuery/DbTx（SQLite/PG/MySQL）
                  方言辅助（sql_concat_agg / sql_settings_upsert / sql_favicon_upsert）
  models.rs       用户与审计
  auth/           口令哈希（Argon2id）、登录限速、API Token
  bookmarks/      书签核心域：CRUD/标签/搜索表达式/批量/Bundle/自动打标/Netscape
  capture/        网页抓取：元数据/快照/Archive 回查/SSRF 防护
  tasks/          进程内任务队列（metadata/snapshot/archive_lookup/preview）
  web/            axum 路由与各 API 模块；前端静态资源 rust-embed 嵌入
frontend/astro/   无框架静态多页（login/bookmarks/detail/shared/settings/admin）
migrations/       迁移（三方言目录）：sqlite/ + postgres/ + mysql/
```

## 测试

每个功能实现配套单元测试，`cargo test` 全量通过是提交前提（CI 参数见
`.github/workflows/ci.yml`）：

```bash
just check        # fmt + clippy + 单元测试（与 CI 逐字对齐）
cargo test        # 全量单元/集成测试
just smoke        # 端到端冒烟（release 构建 + 临时数据目录 + 全链路断言）
```

三数据库一致性测试：核心路径的同一断言集在 `src/db.rs`（`assert_core_paths`）；
设置 `LINKLIST_TEST_DATABASE_URL`（PostgreSQL）/ `LINKLIST_TEST_MYSQL_URL`
（MySQL，指向已建空的测试库）后随 `cargo test` 自动执行，未设置时打印提示跳过。

## 开发规范

- 开发规范见 `AGENTS.md` 与 `SPEC.md`：涉及数据库的变更必须三后端等价、
  迁移必须同时提供三种方言文件；前端交互禁用原生 `alert/confirm/prompt`。
- 前端构建产物 `frontend/dist` 编译期嵌入，不进版本库。

## 文档站点

本站由 zensical 构建（源 `docs/`，产物 `site/` 不进版本库）：

```bash
just docs-build   # 构建到 site/（zensical build --clean）
just docs-serve   # 本地预览 http://127.0.0.1:8000（文件变更自动重建）
```

功能行为、配置项、部署方式变化时同步更新对应文档页面（涉及数据库的内容
必须覆盖三种后端）。
