# Docker 部署

## 镜像构建

```bash
# 标准镜像（多阶段构建：Node 构建前端 + Rust 构建后端）
docker build -f docker/Dockerfile -t linklist .

# 国内网络镜像（走代理拉取基础镜像）
docker build -f docker/CN.Dockerfile -t linklist .
# 或 just docker-build
```

## 三种数据库形态

基础编排 `docker/compose.yaml` 为 SQLite 单容器形态；PostgreSQL / MySQL
形态通过**叠加 overlay 编排文件**启用（官方数据库镜像 + 健康检查 +
持久化卷，linklist 仅在数据库健康后启动）：

```bash
# SQLite（默认形态）
docker compose -f docker/compose.yaml up -d

# PostgreSQL（官方 postgres 镜像）
docker compose -f docker/compose.yaml -f docker/compose.postgres.yaml up -d

# MySQL（官方 mysql 镜像 8.4）
docker compose -f docker/compose.yaml -f docker/compose.mysql.yaml up -d
```

just 任务：`just docker-up` / `just docker-up-postgres` /
`just docker-up-mysql`；停止：`just docker-down` /
`just docker-down-postgres` / `just docker-down-mysql`。

## 数据库凭据

overlay 通过 Compose 变量插值读取凭据（优先级：shell 环境 >
`docker/.env` > overlay 内默认值）。建议把凭据写入 `docker/.env`：

```bash
# docker/.env（PostgreSQL 形态）
POSTGRES_USER=linklist
POSTGRES_PASSWORD=change-me
POSTGRES_DB=linklist

# docker/.env（MySQL 形态；密码请用字母数字，避免连接串特殊字符转义问题）
MYSQL_USER=linklist
MYSQL_PASSWORD=change-me
MYSQL_DATABASE=linklist
MYSQL_ROOT_PASSWORD=change-me-root
```

生产部署务必覆盖默认凭据。中国网络环境拉取镜像遵循仓库 docker mtrans
约定：`docker mtrans spull postgres:17` / `docker mtrans spull mysql:8.4`。

## 数据与卷

- **linklist 容器**：数据目录挂载在 `../data:/app/data`——无论使用哪种
  数据库，网页快照、预览图、favicon 缓存与上传资产都写此目录。
- **SQLite 形态**：数据库文件也在该目录（`linklist.db`）。
- **PostgreSQL / MySQL 形态**：数据库数据分别在命名卷 `postgres_data` /
  `mysql_data` 中。

SQLite 形态需要目录归属容器运行用户（uid 65532）：

```bash
mkdir -p data && chown -R 65532:65532 ./data
```

## 容器环境变量

容器内无需配置文件，所有配置项通过 `LINKLIST_*` 环境变量注入（优先级高于
config.toml，见[配置参考](02-configuration.md)）。常用：

```yaml
environment:
  LINKLIST_GUEST_SHARING: "true"         # 允许匿名访客查看公开共享书签
  LINKLIST_LOG: info
```

> 账号规则：容器首启不预置任何账号。首个打开 Web 界面完成初始化/登录的
> 账号（含 OIDC / 反向代理认证）自动成为超级用户；忘记口令时在容器内执行
> `linklist reset-password <username>` 重置。

## 备份与恢复

三种数据库的备份/恢复命令、数据目录（快照与资产）打包、跨库迁移与定期
备份建议，见[备份与恢复](05-backup-restore.md)。要点：数据库与 `data/`
目录文件必须成对备份；恢复前先停 linklist 容器。
