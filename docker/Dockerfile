# linklist Docker 多阶段构建。构建上下文是本仓库根（含 src/、frontend/、Cargo.toml）。
# 本文件全部使用官方源（apt/cargo/rustup/npm 均为国际默认），供 CI 与上线构建；
# 中国网络环境请用 docker/CN.Dockerfile（全量国内镜像源），二者除镜像源配置外完全一致。
#
# 与前端的关系：前端由 rust-embed 在编译期嵌入二进制（src/web/frontend.rs 嵌入
# frontend/dist），因此前端构建产物必须先于 cargo build 进入 Rust 构建阶段。
#
# 运行阶段用 distroless/static：全程 rustls（reqwest rustls-tls，无 OpenSSL），
# 依赖均为纯 Rust，musl 静态链接后不依赖 glibc；static 变体自带 ca-certificates
# 与 tzdata，出站 HTTPS（抓取书签元数据）的根证书无需额外安装。

# ---- 前端 builder：Astro 静态产物（编译期嵌入用） ----
FROM node:24-slim AS frontend-builder

# pnpm 版本由 frontend/astro/package.json 的 packageManager 决定。
RUN corepack enable

WORKDIR /app/frontend/astro
COPY frontend/astro/package.json frontend/astro/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile

COPY frontend/astro/ .
RUN pnpm build
# Astro outDir=../dist：产物落在 /app/frontend/dist，供 rust-embed 嵌入。

# ---- Rust builder：musl 静态链接 ----
FROM rust:1-slim AS builder

# buildx 会按目标平台注入 TARGETPLATFORM（linux/amd64、linux/arm64）；
# 预定义 ARG 必须显式声明才能在 RUN 中引用。
ARG TARGETPLATFORM

# musl-gcc 供链接器使用。
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools pkg-config \
    && rm -rf /var/lib/apt/lists/*

# 平台 → musl target 一一对应（不做交叉编译，CI 中每台 runner 各构建一个平台）。
RUN case "$TARGETPLATFORM" in \
        linux/amd64) RUST_TARGET=x86_64-unknown-linux-musl ;; \
        linux/arm64) RUST_TARGET=aarch64-unknown-linux-musl ;; \
        *) echo "不支持的构建平台：$TARGETPLATFORM" >&2; exit 1 ;; \
    esac \
    && rustup target add "$RUST_TARGET" \
    && printf '%s\n' "$RUST_TARGET" > /.rust-target \
    && printf 'export CARGO_TARGET_%s_LINKER=musl-gcc\n' \
        "$(printf '%s' "$RUST_TARGET" | tr 'a-z-' 'A-Z_')" > /.rust-env

WORKDIR /app
COPY . .

# 前端产物进入源码树，rust-embed 编译时嵌入（上下文中 frontend/dist 被
# .dockerignore 排除，避免用本地过期产物）。
COPY --from=frontend-builder /app/frontend/dist ./frontend/dist

# 依赖与产物目录挂 cache 卷：源码不变时跨构建复用，改动时只重编译受影响的 crate。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    . /.rust-env \
    && cargo build --release --target "$(cat /.rust-target)" \
    && cp "target/$(cat /.rust-target)/release/linklist" /linklist \
    && mkdir /runtime-app

# ---- 运行阶段：distroless static（非 root） ----
FROM gcr.io/distroless/static-debian13:nonroot

COPY --from=builder /linklist /usr/bin/linklist

# 数据目录挂卷持久化（SQLite 数据库、快照、密钥）。不注入 LINKLIST_LISTEN /
# LINKLIST_DATA_DIR：监听地址用默认值 0.0.0.0:9090，数据目录用默认值 ./data
# （工作目录下）。工作目录为 /app 且属主为运行用户（65532），数据落在 /app/data，
# 容器内无需任何环境变量即可启动；其余可选配置通过 LINKLIST_* 环境变量注入。
WORKDIR /app
COPY --from=builder --chown=65532:65532 /runtime-app /app
EXPOSE 9090
VOLUME ["/app/data"]

ENTRYPOINT ["linklist", "run"]
