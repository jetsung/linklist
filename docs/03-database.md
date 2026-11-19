# 数据库

LinkList 支持 **SQLite（默认）/ PostgreSQL / MySQL** 三种数据库。连接通过
单一连接串配置：环境变量 `LINKLIST_DATABASE_URL`（项目约定名）或通用别名
`DATABASE_URL`（生态通行惯例，`DATABASE_URL=… linklist init` 可直接工作，
前者优先），或配置文件 `database_url` 字段。

## 后端选择规则

连接串**前缀决定后端**，三种后端使用同一配置字段：

| 前缀 | 后端 |
|---|---|
| `postgres://`、`postgresql://` | PostgreSQL |
| `mysql://`、`mariadb://` | MySQL |
| 其他 / 留空 | SQLite（默认） |

```bash
# SQLite（默认，留空即可；显式指定路径时）
export LINKLIST_DATABASE_URL="sqlite://data/linklist.db?mode=rwc"

# PostgreSQL
export LINKLIST_DATABASE_URL="postgres://user:pass@host:5432/linklist"

# MySQL（推荐 utf8mb4 连接参数）
export LINKLIST_DATABASE_URL="mysql://user:pass@host:3306/linklist?characterEncoding=utf8mb4"
```

未设置任何连接串时零配置运行：在数据目录下自动创建 SQLite
（`linklist.db`）并执行迁移。配置了不属于三种受支持后端的协议（如
`mssql://`）会拒绝启动并给出明确错误。

## 迁移行为

- 首次连接（`run` 启动或 `linklist init`）自动执行对应方言的迁移。
- SQLite 迁移走版本表机制（`migrations/sqlite/`）；PostgreSQL 与 MySQL 为
  幂等 SQL（`migrations/postgres/`、`migrations/mysql/`），重复启动不重复建表。
- 三份迁移创建的表结构语义等价（列/主键/唯一约束一致）。

## 方言等价说明

同一功能在三后端下行为一致；个别实现差异：

- **标签大小写不敏感唯一**：SQLite 用触发器（ASCII 折叠）、PostgreSQL 用
  触发器 + `LOWER()`、MySQL 依赖表级 `utf8mb4_0900_ai_ci` 排序规则（对
  Unicode 折叠更宽，例如重音字符也视为相同——方向更严格，不影响"防撞名"）。
- **全文检索**：主搜索路径为 `LOWER(col) LIKE ?` 的icontains 语义（对齐
  上游），不依赖 FTS 引擎；SQLite 额外建了 FTS5 虚拟表，MySQL 建了
  InnoDB FULLTEXT 索引（ngram 分词）供未来扩展。
- **布尔与时间**：布尔比较统一 `TRUE/FALSE` 字面量；时间以 ISO 字符串存取，
  三方言时间列均为 TEXT（业务代码字符串读写直通，无跨类型解码差异）。
- **自增主键回读**：SQLite/MySQL 用 INSERT 响应自带回读，PostgreSQL 用
  `RETURNING id`。

## Docker 部署形态

三种数据库在 Docker Compose 中对应三种形态（overlay 文件切换，官方镜像 +
健康检查 + 持久化卷）：

```bash
# SQLite（默认形态，无需额外服务）
docker compose -f docker/compose.yaml up -d

# PostgreSQL（官方 postgres 镜像）
docker compose -f docker/compose.yaml -f docker/compose.postgres.yaml up -d

# MySQL（官方 mysql 镜像 8.4 LTS）
docker compose -f docker/compose.yaml -f docker/compose.mysql.yaml up -d
```

或用 just 任务：`just docker-up` / `just docker-up-postgres` /
`just docker-up-mysql`。凭据与环境变量见 [Docker 部署](04-docker.md)。

## 连接 MySQL 的推荐参数

- `characterEncoding=utf8mb4`：保证中文与 Emoji 正常存取。
- 服务端建议 `--character-set-server=utf8mb4 --collation-server=utf8mb4_0900_ai_ci`
  （Docker 形态的 overlay 已内置）。
- 需要 MySQL 8.0.16+（CHECK 约束强制执行、TEXT 表达式默认值）。
