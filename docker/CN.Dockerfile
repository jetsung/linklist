# 中国网络环境专用构建文件：apt 换阿里云镜像，cargo/rustup 走 rsproxy，
# npm/pnpm 走 npmmirror。配置只写入 builder 层，最终运行镜像不含任何镜像源或
# 代理配置。国际/上线构建请用 docker/Dockerfile（官方源），二者除镜像源
# 配置外完全一致：
#
#   docker build -f docker/CN.Dockerfile -t linklist:dev .   # 中国网络环境
#   docker build -f docker/Dockerfile    -t linklist:dev .   # 国际/上线构建

# ---- 前端 builder：Astro 静态产物（编译期嵌入用） ----
FROM node:24-slim AS frontend-builder

RUN corepack enable

WORKDIR /app/frontend/astro
COPY frontend/astro/package.json frontend/astro/pnpm-lock.yaml ./
# npmmirror 加速依赖安装（仅进 builder 层）。
RUN npm config set registry https://registry.npmmirror.com \
    && pnpm install --frozen-lockfile

COPY frontend/astro/ .
RUN pnpm build
# Astro outDir=../dist：产物落在 /app/frontend/dist，供 rust-embed 嵌入。

# ---- Rust builder：musl 静态链接 ----
FROM rust:1-slim AS builder

ARG TARGETPLATFORM

# apt 换阿里云镜像；cargo 走 rsproxy（sparse index），拉依赖不走外网。
RUN sed -i 's|deb.debian.org|mirrors.aliyun.com|g' \
        /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list 2>/dev/null || true \
    && printf '[source.crates-io]\nreplace-with = "rsproxy"\n\n[source.rsproxy]\nregistry = "sparse+https://rsproxy.cn/index/"\n\n[net]\ngit-fetch-with-cli = true\n' \
        > /usr/local/cargo/config.toml

RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools pkg-config \
    && rm -rf /var/lib/apt/lists/*

# rustup 下载 musl target 组件走 rsproxy 镜像。
ENV RUSTUP_DIST_SERVER=https://rsproxy.cn \
    RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup

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
