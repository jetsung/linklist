#!/usr/bin/env bash
# linklist 端到端冒烟：release 构建 + 临时数据目录启动 + 全链路断言。
# 用法: bash scripts/smoke.sh   （结束后自动清理临时目录与进程）
set -euo pipefail

PORT=${SMOKE_PORT:-9299}
BASE="http://127.0.0.1:${PORT}"
TMP=$(mktemp -d /tmp/linklist-smoke.XXXXXX)
BIN=${SMOKE_BIN:-$(cargo metadata --format-version 1 2>/dev/null | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')/release/linklist}

cleanup() {
  [[ -n "${SRV_PID:-}" ]] && kill "$SRV_PID" 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

echo "==> 构建 release"
cargo build --release

echo "==> 启动（数据目录 ${TMP}）"
LINKLIST_DATA_DIR="$TMP" LINKLIST_LISTEN="127.0.0.1:${PORT}" "$BIN" run >"$TMP/server.log" 2>&1 &
SRV_PID=$!
for i in $(seq 1 30); do
  curl -sf "$BASE/api/v1/auth/status" > /dev/null && break
  sleep 1
done

pass=0; fail=0
check() { # check <name> <expected> <actual>
  if [[ "$2" == "$3" ]]; then pass=$((pass+1)); echo "  ✓ $1 ($3)";
  else fail=$((fail+1)); echo "  ✗ $1: 期望 $2 实际 $3"; fi
}
code() { curl -s -o /dev/null -w "%{http_code}" "$@"; }

echo "==> 页面路由"
for p in / /login /bookmarks /detail /shared /settings /admin; do
  check "页面 $p" 200 "$(code "$BASE$p")"
done
check "静态 CSS" 200 "$(code "$BASE/assets/app.css")"
check "未知资源 404" 404 "$(code "$BASE/assets/nope.js")"

echo "==> 认证"
check "status" 200 "$(code "$BASE/api/v1/auth/status")"
check "setup" 201 "$(code -X POST "$BASE/api/v1/auth/setup" -H 'Content-Type: application/json' -d '{"username":"admin","password":"password123"}')"
check "login" 200 "$(code -c "$TMP/cookies" -X POST "$BASE/api/v1/auth/login" -H 'Content-Type: application/json' -d '{"username":"admin","password":"password123"}')"
check "me" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/auth/me")"

echo "==> 书签 CRUD + 抓取回填"
BID=$(curl -s -b "$TMP/cookies" -X POST "$BASE/api/v1/bookmarks" -H 'Content-Type: application/json' -d '{"url":"https://example.com/"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["id"])')
check "创建书签 id>0" True "$([ "$BID" -gt 0 ] && echo True)"
check "capture 入队" 202 "$(code -b "$TMP/cookies" -X POST "$BASE/api/v1/bookmarks/$BID/capture" -H 'Content-Type: application/json' -d '{}')"
sleep 8
TITLE=$(curl -s -b "$TMP/cookies" "$BASE/api/v1/bookmarks/$BID" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("title",""))')
check "抓取回填标题" "Example Domain" "$TITLE"

echo "==> 标签/批量/归档/搜索"
check "写标签" 200 "$(code -b "$TMP/cookies" -X PUT "$BASE/api/v1/bookmarks/$BID/tags" -H 'Content-Type: application/json' -d '{"tags":["rust","web"]}')"
check "表达式搜索" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/bookmarks?q=tag%3Arust")"
check "批量加标签" 200 "$(code -b "$TMP/cookies" -X POST "$BASE/api/v1/bookmarks/bulk/tags/add" -H 'Content-Type: application/json' -d "{\"ids\":[$BID],\"tags\":[\"extra\"]}")"
check "批量标已读" 200 "$(code -b "$TMP/cookies" -X POST "$BASE/api/v1/bookmarks/bulk/mark-read" -H 'Content-Type: application/json' -d "{\"ids\":[$BID]}")"
check "PATCH 归档" 200 "$(code -b "$TMP/cookies" -X PATCH "$BASE/api/v1/bookmarks/$BID" -H 'Content-Type: application/json' -d '{"is_archived":true}')"
ARCH=$(curl -s -b "$TMP/cookies" "$BASE/api/v1/bookmarks?filter=archived" | python3 -c 'import json,sys;print(json.load(sys.stdin)["total"])')
check "归档隔离可见" 1 "$ARCH"

echo "==> 导出 / Token / 共享 / RSS"
check "导出" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/bookmarks/export")"
TOK=$(curl -s -b "$TMP/cookies" -X POST "$BASE/api/v1/auth/tokens" -H 'Content-Type: application/json' -d '{"name":"smoke"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["token"])')
check "Token 调用" 200 "$(code -H "Authorization: Token $TOK" "$BASE/api/v1/bookmarks")"
check "公开共享(关)" 401 "$(code "$BASE/api/v1/shared/public")"
check "设 guest_sharing" 200 "$(code -b "$TMP/cookies" -X PUT "$BASE/api/v1/admin/settings" -H 'Content-Type: application/json' -d '{"guest_sharing":"true"}')"
check "公开共享(开)" 200 "$(code "$BASE/api/v1/shared/public")"
check "RSS" 200 "$(code "$BASE/feed/public_shared")"

echo "==> 管理面板"
check "用户列表" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/admin/users")"
check "原始数据" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/admin/data/bookmarks")"
check "任务列表" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/admin/tasks")"
check "审计日志" 200 "$(code -b "$TMP/cookies" "$BASE/api/v1/admin/audit-logs")"

echo
echo "冒烟结果: $pass 通过, $fail 失败"
[[ $fail -eq 0 ]]
