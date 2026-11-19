# linklist 本地开发任务。
#
# 工具链（rust / node / pnpm / just 等）通过 mise 安装。根目录无 package.json /
# pnpm-lock.yaml / build.mjs：前端构建直接调 frontend/astro 的 pnpm，不经根目录转发。
# 注意：cargo build 会经 rust-embed 在编译期嵌入 frontend/dist（src/web/frontend.rs），
# 改完前端先 `just frontend-build`，否则跑的是旧页面。

# 默认：列出全部任务
default:
    @just --list

# ---- 后端（默认监听 0.0.0.0:9090，数据目录 ./data）----

# 启动后端开发服务（Web 界面：http://127.0.0.1:9090）
# 可附带参数，例如：just backend -- --listen 127.0.0.1:9091
[group('backend')]
@backend *args:
    cargo run -- run {{ args }}

# 构建发布版（先构建前端产物，rust-embed 编译期需要 frontend/dist）
[group('backend')]
@build:
    just frontend-build
    cargo build --release

# ---- 前端（frontend/astro/，Astro 静态多页）----

# 安装前端依赖（frontend/astro/，首次使用或 pnpm-lock.yaml 变更后）
[group('frontend')]
@frontend-install:
    pnpm -C frontend/astro install

# 启动前端 dev server（Astro 默认 http://localhost:4321；静态多页 + 本地 API 直连 9090）
[group('frontend')]
@frontend:
    exec pnpm -C frontend/astro dev

# 构建前端静态产物到 frontend/dist（rust-embed 编译期依赖）
[group('frontend')]
@frontend-build:
    pnpm -C frontend/astro build

# ---- 检查（与 .github/workflows/ci.yml 参数逐字对齐）----

# 全量检查：fmt + clippy + 单元测试（提交/推送前建议跑一遍）
[group('check')]
@check:
    just frontend-build
    cargo fmt --all -- --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# 只跑后端测试（需先有 frontend/dist，缺失时自动构建）
[group('check')]
@test:
    just frontend-build
    cargo test

# 格式化代码（rustfmt.toml 对齐 Google Rust 风格指南）
[group('check')]
@fmt:
    cargo fmt --all

# 只跑 clippy lint（-D warnings，含 Cargo.toml [lints] 强制项）
[group('check')]
@lint:
    cargo clippy --all-targets -- -D warnings

# ---- 端到端冒烟（scripts/smoke.sh：release 构建 + 临时数据目录 + 全链路断言）----

[group('check')]
@smoke:
    bash scripts/smoke.sh

# ---- Docker（docker/Dockerfile 官方源 / docker/CN.Dockerfile 国内源）----

# 本地构建镜像（中国网络环境走 CN.Dockerfile），产物 linklist:dev
[group('docker')]
@docker-build *args:
    docker build -f docker/CN.Dockerfile -t linklist:dev {{ args }} .

# 本地起服务（默认 SQLite 形态）：docker compose -f docker/compose.yaml up -d
[group('docker')]
@docker-up *args:
    docker compose -f docker/compose.yaml up -d {{ args }}

# 以 PostgreSQL 形态起服务（官方 postgres 镜像，凭据见 compose.postgres.yaml 头注）
[group('docker')]
@docker-up-postgres *args:
    docker compose -f docker/compose.yaml -f docker/compose.postgres.yaml up -d {{ args }}

# 以 MySQL 形态起服务（官方 mysql 镜像 8.4，凭据见 compose.mysql.yaml 头注）
[group('docker')]
@docker-up-mysql *args:
    docker compose -f docker/compose.yaml -f docker/compose.mysql.yaml up -d {{ args }}

# 停止并移除 compose 服务（保留 data/ 数据卷目录）
[group('docker')]
@docker-down:
    docker compose -f docker/compose.yaml down

# 停止并移除对应形态的 compose 服务（含数据库容器与数据卷）
[group('docker')]
@docker-down-postgres:
    docker compose -f docker/compose.yaml -f docker/compose.postgres.yaml down

# 停止并移除 MySQL 形态的 compose 服务（含数据库容器与数据卷）
[group('docker')]
@docker-down-mysql:
    docker compose -f docker/compose.yaml -f docker/compose.mysql.yaml down

# ---- 文档（zensical，源 docs/，产物 site/ 不进版本库）----

# 构建文档站点到 site/（--clean 先清理旧产物）
[group('docs')]
@docs-build *args:
    zensical build --clean {{ args }}

# 本地调试预览文档站点（http://127.0.0.1:8000，文件变更自动重建）
[group('docs')]
@docs-serve *args:
    zensical serve {{ args }}

# ---- 组合 ----

# 同时启动后端与前端本地调试（Ctrl-C 一并退出；任一退出则整体结束）
# 清理用 pkill 按命令行模式兜底：pnpm 会为 astro 另开进程组、agent 环境下 astro
# 还会自动后台化脱离进程组，kill -- -pid 都打不到，只有 pkill 能兜住。
# 注意：前端 dev server 的页面与后端 9090 端口分属两个进程，跨端口联调时
# 需自行处理代理（或在浏览器直接访问 9090 看嵌入产物）。
[group('dev')]
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    cleanup() {
        pkill -TERM -f 'node .*[a]stro' 2>/dev/null || true
        pkill -TERM -f 'pnpm -C [f]rontend' 2>/dev/null || true
        pkill -TERM -f 'linklis[t] run' 2>/dev/null || true
        pkill -TERM -f 'just (fronten[d]|backen[d])' 2>/dev/null || true
    }
    trap cleanup EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
    # 启动前先强杀遗留的 debug 后端二进制（target/debug/linklist），避免 9090 端口被占。
    pkill -9 -f '[d]ebug/linklist' 2>/dev/null || true
    just backend & pids+=("$!")
    just frontend & pids+=("$!")
    wait -n
    wait

# 匹配串用 [x] 写法避免 pkill 命中本进程自身。
# 停止所有本地开发服务（后端 9090 与前端 432x 的 astro/pnpm/cargo 进程）
[group('dev')]
@stop:
    (cd frontend/astro && pnpm exec astro dev stop >/dev/null 2>&1) || true
    pkill -TERM -f 'node .*[a]stro' 2>/dev/null || true
    pkill -TERM -f 'pnpm -C [f]rontend' 2>/dev/null || true
    pkill -TERM -f 'linklis[t] run' 2>/dev/null || true
    pkill -9 -f '[d]ebug/linklist' 2>/dev/null || true
    pkill -TERM -f 'just (de[v]|fronten[d]|backen[d])' 2>/dev/null || true
    sleep 1
    @echo "本地开发服务已停止"
