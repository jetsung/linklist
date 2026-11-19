# LinkList

LinkList 是用 Rust 重写的自托管书签管理服务（linkding 的 Rust 版）：单二进制交付，
无 Python/Node 运行时依赖，前端静态资源编译期嵌入；支持 **SQLite（默认）/
PostgreSQL / MySQL** 三种数据库，通过 `DATABASE_URL` 连接串选择后端。

## 能力一览

- **书签核心**：书签 CRUD 与 URL 判重、标签体系、表达式搜索（`tag:rust`、
  `is:unread`、`#下载`、AND/OR/NOT）、批量操作、归档视图、Netscape 导入/导出、
  RSS 订阅源。
- **内容抓取**：网页元数据自动抓取（标题/描述/图标/预览图）、Archive.today
  快照、Internet Archive 可用性回查、singlefile 本地快照、资产上传。
- **多用户与共享**：注册（首个用户自动成为管理员）、会话 + API Token 认证、
  共享书签双层可见性（登录共享视图 + 匿名公开共享）、审计日志与登录限速。
- **三数据库后端**：SQLite（零配置默认）/ PostgreSQL / MySQL 任选其一，
  迁移与业务行为在后端之间等价。
- **单二进制部署**：`linklist init` 一条命令完成项目初始化；官方 Docker 编排
  提供三种数据库形态（overlay 文件切换）。
- **可观测与运维**：结构化日志（RUST_LOG 语法）、管理面板（用户/任务队列/
  全局设置/审计）、CLI 工具（init / reset-password / version / help）。

## 阅读路径

| 我想…… | 看这里 |
|---|---|
| 尽快跑起来 | [快速开始](01-getting-started.md) |
| 配置监听/数据目录/日志 | [配置参考](02-configuration.md) |
| 选择并连接数据库（含 MySQL） | [数据库](03-database.md) |
| 用 Docker 部署与备份 | [Docker 部署](04-docker.md) |
| 日常使用（搜索/标签/快照） | [功能使用](06-features.md) |
| 对接或二次开发 API | [REST API](08-api.md) / [本地开发](09-development.md) |
| 配置单点登录（OIDC） | [OIDC 单点登录](07-oidc.md) |
| 备份与恢复数据 | [备份与恢复](05-backup-restore.md) |
| 了解与 linkding 的能力差异 | [上游对齐](10-linkding-alignment.md) |

## 与 linkding 的关系

LinkList 以 linkding v1.47.0 为上游参考实现，按"能力对齐而非实现一致"的
口径逐域对齐其行为（书签模型、搜索表达式、共享语义、用户偏好等），并把
交付形态改为 Rust 单二进制 + 前端静态嵌入。核心能力已全部对齐，历史缺口
（导出保真度、singlefile 上传、OIDC/反代/CORS 等）均已补齐，另有 13+ 项
扩展（管理端 REST、审计日志、登录限速、Token 安全增强、三数据库后端等）。
详细对照见[上游对齐](10-linkding-alignment.md)。
