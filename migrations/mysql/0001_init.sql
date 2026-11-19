-- linklist MySQL 初始表结构（与 SQLite / PostgreSQL 的 0001_init.sql 语义对齐）。
-- 本文件由应用侧 raw_sql 执行，全部使用 CREATE TABLE IF NOT EXISTS 保证可重复执行。
-- 注意：MySQL 无 `CREATE INDEX IF NOT EXISTS`，二级索引一律并入表定义（INDEX 子句）。
--
-- 方言映射（design D2）：
-- * 主键：INTEGER AUTOINCREMENT / BIGSERIAL → BIGINT AUTO_INCREMENT（有符号，与 i64 对齐）。
-- * 布尔：INTEGER 0/1 / BOOLEAN → TINYINT(1)。
-- * 时间：SQLite TEXT / PG TIMESTAMPTZ → TEXT（业务代码以字符串 ISO 格式读写，
--   TEXT 与现有读写路径完全对齐，避免 DATETIME 的类型解码差异）。
-- * 全文检索：SQLite FTS5 虚拟表+触发器 → InnoDB FULLTEXT 索引（自动同步，
--   ngram 分词器兼容中日文；业务搜索主路径为 LIKE icontains，见 src/bookmarks/search.rs）。
-- * 标签大小写不敏感唯一：SQLite/PG 用触发器，MySQL 表级 collation（utf8mb4_0900_ai_ci）
--   天然大小写不敏感，UNIQUE 约束直接覆盖（对 Unicode 折叠更宽于 SQLite 的 ASCII-only
--   NOCASE，属可接受的方言差异）。
-- * 可索引的文本列用 VARCHAR 定长（TEXT 列无法直接建索引），长度对齐 linkding/Django 字段。
-- 需要 MySQL 8.0.16+（CHECK 强制执行、TEXT 表达式默认值）。

CREATE TABLE IF NOT EXISTS users (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    username VARCHAR(150) NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_admin TINYINT(1) NOT NULL DEFAULT 0,
    is_active TINYINT(1) NOT NULL DEFAULT 1,
    date_joined TEXT NOT NULL
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS user_profiles (
    user_id BIGINT PRIMARY KEY,
    theme VARCHAR(32) NOT NULL DEFAULT ('auto'),
    items_per_page INT NOT NULL DEFAULT 30 CHECK (items_per_page >= 10),
    tag_grouping VARCHAR(32) NOT NULL DEFAULT ('alphabetical'),
    enable_favicons TINYINT(1) NOT NULL DEFAULT 1,
    enable_preview_images TINYINT(1) NOT NULL DEFAULT 0,
    enable_automatic_snapshots TINYINT(1) NOT NULL DEFAULT 0,
    enable_sharing TINYINT(1) NOT NULL DEFAULT 0,
    enable_public_sharing TINYINT(1) NOT NULL DEFAULT 0,
    auto_tagging_rules TEXT NOT NULL DEFAULT (''),
    custom_css TEXT NOT NULL DEFAULT (''),
    search_preferences TEXT NOT NULL DEFAULT ('{}'),
    -- 对齐 linkding 用户资料设置项：日期显示、描述显示、操作按钮、分页、搜索等。
    bookmark_date_display VARCHAR(32) NOT NULL DEFAULT ('relative'),
    bookmark_description_display VARCHAR(32) NOT NULL DEFAULT ('inline'),
    bookmark_description_max_lines INT NOT NULL DEFAULT 5,
    display_url TINYINT(1) NOT NULL DEFAULT 1,
    permanent_notes TINYINT(1) NOT NULL DEFAULT 0,
    display_view_bookmark_action TINYINT(1) NOT NULL DEFAULT 1,
    display_edit_bookmark_action TINYINT(1) NOT NULL DEFAULT 1,
    display_archive_bookmark_action TINYINT(1) NOT NULL DEFAULT 1,
    display_remove_bookmark_action TINYINT(1) NOT NULL DEFAULT 1,
    bookmark_link_target VARCHAR(16) NOT NULL DEFAULT ('_blank'),
    sticky_pagination TINYINT(1) NOT NULL DEFAULT 0,
    collapse_side_panel TINYINT(1) NOT NULL DEFAULT 0,
    hide_bundles TINYINT(1) NOT NULL DEFAULT 0,
    tag_search VARCHAR(16) NOT NULL DEFAULT ('strict'),
    legacy_search TINYINT(1) NOT NULL DEFAULT 0,
    web_archive_integration VARCHAR(32) NOT NULL DEFAULT ('disabled'),
    default_mark_unread TINYINT(1) NOT NULL DEFAULT 0,
    default_mark_shared TINYINT(1) NOT NULL DEFAULT 1,
    CONSTRAINT fk_profiles_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS api_tokens (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    name TEXT NOT NULL DEFAULT (''),
    token_hash VARCHAR(64) NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT,
    expires_at TEXT,
    CONSTRAINT fk_tokens_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS feed_tokens (
    user_id BIGINT PRIMARY KEY,
    `key` VARCHAR(64) NOT NULL UNIQUE,
    CONSTRAINT fk_feed_tokens_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS bookmarks (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    url TEXT NOT NULL,
    url_hash VARCHAR(64) NOT NULL,
    title TEXT NOT NULL DEFAULT (''),
    description TEXT NOT NULL DEFAULT (''),
    notes TEXT NOT NULL DEFAULT (''),
    is_unread TINYINT(1) NOT NULL DEFAULT 0,
    is_archived TINYINT(1) NOT NULL DEFAULT 0,
    is_shared TINYINT(1) NOT NULL DEFAULT 0,
    favicon_path TEXT,
    preview_image_path TEXT,
    snapshot_path TEXT,
    snapshot_date TEXT,
    archive_url TEXT,
    archive_date TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    accessed_at TEXT,
    UNIQUE KEY uk_bookmarks_user_urlhash (user_id, url_hash),
    KEY idx_bookmarks_user (user_id),
    KEY idx_bookmarks_user_unread (user_id, is_unread),
    KEY idx_bookmarks_user_archived (user_id, is_archived),
    KEY idx_bookmarks_user_shared (user_id, is_shared),
    FULLTEXT KEY ft_bookmarks_content (title, description, notes, url) WITH PARSER ngram,
    CONSTRAINT fk_bookmarks_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS tags (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    name VARCHAR(255) NOT NULL,
    -- collation ai_ci 天然大小写不敏感（替代 SQLite/PG 的 NOCASE 触发器）。
    UNIQUE KEY uk_tags_user_name (user_id, name),
    KEY idx_tags_user (user_id),
    CONSTRAINT fk_tags_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS bookmark_tags (
    bookmark_id BIGINT NOT NULL,
    tag_id BIGINT NOT NULL,
    PRIMARY KEY (bookmark_id, tag_id),
    KEY idx_bookmark_tags_tag (tag_id),
    CONSTRAINT fk_bookmark_tags_bookmark FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE,
    CONSTRAINT fk_bookmark_tags_tag FOREIGN KEY (tag_id) REFERENCES tags(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS bookmark_assets (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    bookmark_id BIGINT NOT NULL,
    asset_type VARCHAR(16) NOT NULL CHECK (asset_type IN ('snapshot', 'upload')),
    file_path TEXT NOT NULL,
    file_size BIGINT NOT NULL DEFAULT 0,
    content_type VARCHAR(255) NOT NULL DEFAULT ('text/html'),
    display_name TEXT NOT NULL DEFAULT (''),
    status VARCHAR(16) NOT NULL DEFAULT ('complete') CHECK (status IN ('pending', 'complete', 'failure')),
    date_created TEXT NOT NULL,
    KEY idx_bookmark_assets_bookmark (bookmark_id),
    CONSTRAINT fk_assets_bookmark FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS bundles (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT NOT NULL,
    name VARCHAR(255) NOT NULL,
    search_json TEXT NOT NULL DEFAULT ('{}'),
    created_at TEXT NOT NULL,
    -- Bundles 拖拽排序：order 列（linkding BookmarkBundle.order 对齐）。
    sort_order INT NOT NULL DEFAULT 0,
    CONSTRAINT fk_bundles_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS capture_tasks (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    bookmark_id BIGINT NOT NULL,
    -- kind 含 'preview'（预览图任务，对齐上游 load_preview_image 后台任务）。
    kind VARCHAR(16) NOT NULL CHECK (kind IN ('metadata', 'snapshot', 'archive_lookup', 'preview')),
    status VARCHAR(16) NOT NULL DEFAULT ('pending') CHECK (status IN ('pending', 'running', 'success', 'failed')),
    attempts INT NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    KEY idx_capture_tasks_bookmark (bookmark_id),
    KEY idx_capture_tasks_status (status),
    CONSTRAINT fk_capture_tasks_bookmark FOREIGN KEY (bookmark_id) REFERENCES bookmarks(id) ON DELETE CASCADE
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS audit_logs (
    id BIGINT AUTO_INCREMENT PRIMARY KEY,
    user_id BIGINT,
    action VARCHAR(64) NOT NULL,
    target TEXT NOT NULL DEFAULT (''),
    created_at TEXT NOT NULL,
    KEY idx_audit_logs_user (user_id),
    CONSTRAINT fk_audit_logs_user FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;

CREATE TABLE IF NOT EXISTS settings (
    `key` VARCHAR(255) PRIMARY KEY,
    value TEXT NOT NULL
) DEFAULT CHARSET = utf8mb4 COLLATE = utf8mb4_0900_ai_ci;
