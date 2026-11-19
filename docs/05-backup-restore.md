# 数据库备份与恢复

本文覆盖三种数据库（SQLite / PostgreSQL / MySQL）的备份与恢复操作，以及
快照、上传资产等**数据目录文件**的备份。无论使用哪种数据库，书签的网页
快照、预览图、favicon 缓存与上传资产都存放在本地数据目录（Compose 默认
挂载仓库根的 `data/`），**必须一并备份**。

## 备份范围清单

| 对象 | 位置 | 说明 |
|---|---|---|
| 数据库 | 见下文按后端操作 | 书签/标签/用户/设置等结构化数据 |
| 数据目录 | `data/`（Compose 挂载到容器 `/app/data`） | 快照 HTML、预览图、favicon 缓存、上传资产；SQLite 模式下还含数据库文件本身 |
| 配置 | `config.toml`、`docker/.env`、systemd 环境文件 | 含数据库凭据与 `LINKLIST_ADMIN_PASSWORD` 等，建议脱敏保存 |

## SQLite（默认）

SQLite 的库文件就是 `data/linklist.db`（WAL 模式，另有 `-wal`/`-shm`
辅助文件）。

**在线备份（推荐，服务不停机）**：

```bash
sqlite3 data/linklist.db ".backup 'backups/linklist-$(date +%F).db'"
```

`.backup` 命令对 WAL 模式的库做一致性快照，备份期间可继续读写。

**停机备份（最简单）**：停止服务后直接打包整个数据目录：

```bash
just docker-down        # 或 systemctl stop linklist
tar czf backups/data-$(date +%F).tar.gz data/
```

**恢复**：

```bash
# 停服后放回数据库文件
cp backups/linklist-2026-01-01.db data/linklist.db
rm -f data/linklist.db-wal data/linklist.db-shm   # 丢弃旧 WAL，启动时自动重建
# 若备份的是整个 data/ 目录，解包恢复即可：
# tar xzf backups/data-2026-01-01.tar.gz
```

恢复后启动服务，迁移幂等执行（已初始化的库不会重复建表），随后建议打开
站点做一次登录与书签列表冒烟。

## PostgreSQL

Docker 形态的容器名为 `linklist-postgres`，库名/用户按 `docker/.env` 中的
`POSTGRES_*` 配置（下例用默认值 `linklist`，生产请替换）。

**备份**（custom 格式，支持压缩与选择性恢复）：

```bash
docker exec linklist-postgres pg_dump -U linklist -d linklist -Fc \
  > backups/linklist-pg-$(date +%F).dump
```

**恢复**：

```bash
# 先停止 linklist 容器，避免恢复期间写入竞争
docker stop linklist

docker exec -i linklist-postgres pg_restore -U linklist -d linklist \
  --clean --if-exists < backups/linklist-pg-2026-01-01.dump

# 恢复数据目录文件（快照/资产）后重启服务
docker start linklist
```

!!! warning "注意"
    `--clean` 会先删除现有对象再重建，**现有数据将被覆盖**；如需保留，
    先把当前库再备份一份。也可恢复到新建空库后切换连接串。

## MySQL

Docker 形态的容器名为 `linklist-mysql`（官方 mysql:8.4 镜像）。

**备份**（`--single-transaction` 对 InnoDB 做一致性快照，不锁表）：

```bash
docker exec linklist-mysql mysqldump -uroot -p'change-me-root' \
  --single-transaction --routines linklist \
  > backups/linklist-mysql-$(date +%F).sql
```

**恢复**：

```bash
docker stop linklist

docker exec -i linklist-mysql mysql -uroot -p'change-me-root' linklist \
  < backups/linklist-mysql-2026-01-01.sql

docker start linklist
```

## 跨数据库迁移

更换数据库后端（如 SQLite → PostgreSQL）时，表结构由三方言迁移自动创建，
**数据搬运没有内置工具**：数据量小可按表用 SQL/CSV 导出再导入（注意各表
自增主键与外键顺序），或使用 pgloader 等第三方工具。迁移后以
`data/` 目录文件保持不动、仅切换 `LINKLIST_DATABASE_URL` 的方式完成切换，
并按上文做一次登录与列表冒烟。

## 定期备份建议

- 用 cron / systemd timer 定时执行上述备份命令（如每日一次），保留
  7–30 天滚动副本；
- 备份文件与服务器**异地/异机存放**；
- 数据目录与数据库备份**成对恢复**（快照/资产在文件系统里，不在数据库中）；
- 定期做恢复演练：在新环境按"停服 → 恢复库 → 恢复 `data/` → 启动 →
  登录并列出书签"完整走一遍，确认备份可用。
