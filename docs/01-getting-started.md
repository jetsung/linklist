# 快速开始

从构建产物（或 Docker 镜像）出发，经 `linklist init` 初始化、启动服务、
注册管理员，完成一次完整可用的部署。

## 前置要求

- 构建：Rust 1.8x+（`rustup` 安装）；前端构建用 Node/pnpm（产物
  `frontend/dist` 会在编译期经 rust-embed 嵌入二进制）。
- 运行：一个可写的数据目录（默认 `./data`）；如连接外部数据库，则该
  PostgreSQL / MySQL 实例已创建好空库与账号。

## 方式一：源码构建

```bash
# 1. 构建前端产物（rust-embed 编译期需要 frontend/dist）
pnpm -C frontend/astro build     # 或 just frontend-build

# 2. 构建二进制
cargo build --release

# 3. 初始化项目（生成 config.toml + 创建数据目录 + 建库迁移）
./target/release/linklist init

# 4. 启动服务（默认 0.0.0.0:9090）
./target/release/linklist run

# 5. 打开 http://localhost:9090/login 完成首次初始化/登录（首个账号自动成为超级用户）
```

> 账号规则：系统不预置任何账号，**首个登录/初始化的账号自动成为超级用户**，
> 其后注册的账号均为普通用户。忘记口令时用 `linklist reset-password <username>`
> 在服务器本机交互式重置。

重复执行 `init` 幂等安全：已存在的配置文件不覆盖、已初始化的库不重复迁移。

## 方式二：Docker

```bash
mkdir -p data && chown -R 65532:65532 ./data
docker compose -f docker/compose.yaml up -d
```

外部数据库（PostgreSQL / MySQL）形态见 [Docker 部署](04-docker.md)。

## 常用命令

```bash
linklist help                       # 子命令与退出码约定（--help / -h 等价）
linklist version                    # 版本号
linklist init                       # 初始化项目（见上）
linklist run                        # 启动 Web 服务（无参数时默认执行）
linklist reset-password <username>  # 交互式重置口令（不回显）
```

## 下一步

- 数据库存放与连接串选择 → [数据库](03-database.md)
- 全量配置项 → [配置参考](02-configuration.md)
- 搜索表达式与日常功能 → [功能使用](06-features.md)
