-- linklist 初始表结构（design D3）+ 用户资料设置 + Bundle 排序 + 标签 NOCASE 唯一 + 预览图任务
-- （初始版本将 0001~0005 合并为单一迁移；此后发布版本起迁移文件不再合并。）

CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    is_admin INTEGER NOT NULL DEFAULT 0,
    is_active INTEGER NOT NULL DEFAULT 1,
    date_joined TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS user_profiles (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    theme TEXT NOT NULL DEFAULT 'auto',
    items_per_page INTEGER NOT NULL DEFAULT 30 CHECK (items_per_page >= 10),
    tag_grouping TEXT NOT NULL DEFAULT 'alphabetical',
    enable_favicons INTEGER NOT NULL DEFAULT 1,
    enable_preview_images INTEGER NOT NULL DEFAULT 0,
    enable_automatic_snapshots INTEGER NOT NULL DEFAULT 0,
    enable_sharing INTEGER NOT NULL DEFAULT 0,
    enable_public_sharing INTEGER NOT NULL DEFAULT 0,
    auto_tagging_rules TEXT NOT NULL DEFAULT '',
    custom_css TEXT NOT NULL DEFAULT '',
    search_preferences TEXT NOT NULL DEFAULT '{}',
    -- 对齐 linkding 用户资料设置项：日期显示、描述显示、操作按钮、分页、搜索等。
    bookmark_date_display TEXT NOT NULL DEFAULT 'relative',
    bookmark_description_display TEXT NOT NULL DEFAULT 'inline',
    bookmark_description_max_lines INTEGER NOT NULL DEFAULT 5,
    display_url INTEGER NOT NULL DEFAULT 1,
    permanent_notes INTEGER NOT NULL DEFAULT 0,
    display_view_bookmark_action INTEGER NOT NULL DEFAULT 1,
    display_edit_bookmark_action INTEGER NOT NULL DEFAULT 1,
    display_archive_bookmark_action INTEGER NOT NULL DEFAULT 1,
    display_remove_bookmark_action INTEGER NOT NULL DEFAULT 1,
    bookmark_link_target TEXT NOT NULL DEFAULT '_blank',
    sticky_pagination INTEGER NOT NULL DEFAULT 0,
    collapse_side_panel INTEGER NOT NULL DEFAULT 0,
    hide_bundles INTEGER NOT NULL DEFAULT 0,
    tag_search TEXT NOT NULL DEFAULT 'strict',
    legacy_search INTEGER NOT NULL DEFAULT 0,
    web_archive_integration TEXT NOT NULL DEFAULT 'disabled',
    default_mark_unread INTEGER NOT NULL DEFAULT 0,
    default_mark_shared INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS api_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL DEFAULT '',
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_used_at TEXT,
    revoked_at TEXT,
    expires_at TEXT
);

CREATE TABLE IF NOT EXISTS feed_tokens (
    user_id INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    key TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS bookmarks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    url_hash TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    is_unread INTEGER NOT NULL DEFAULT 0,
    is_archived INTEGER NOT NULL DEFAULT 0,
    is_shared INTEGER NOT NULL DEFAULT 0,
    favicon_path TEXT,
    preview_image_path TEXT,
    snapshot_path TEXT,
    snapshot_date TEXT,
    archive_url TEXT,
    archive_date TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    accessed_at TEXT,
    UNIQUE (user_id, url_hash)
);

CREATE TABLE IF NOT EXISTS tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    UNIQUE (user_id, name)
);

CREATE TABLE IF NOT EXISTS bookmark_tags (
    bookmark_id INTEGER NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (bookmark_id, tag_id)
);

CREATE TABLE IF NOT EXISTS bookmark_assets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bookmark_id INTEGER NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    asset_type TEXT NOT NULL CHECK (asset_type IN ('snapshot', 'upload')),
    file_path TEXT NOT NULL,
    file_size INTEGER NOT NULL DEFAULT 0,
    content_type TEXT NOT NULL DEFAULT 'text/html',
    display_name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'complete' CHECK (status IN ('pending', 'complete', 'failure')),
    date_created TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    search_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    -- Bundles 拖拽排序：order 列（linkding BookmarkBundle.order 对齐）。
    sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE VIRTUAL TABLE IF NOT EXISTS bookmarks_fts USING fts5(
    title, description, notes, url,
    content='bookmarks', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS bookmarks_fts_insert AFTER INSERT ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(rowid, title, description, notes, url)
    VALUES (new.id, new.title, new.description, new.notes, new.url);
END;

CREATE TRIGGER IF NOT EXISTS bookmarks_fts_delete AFTER DELETE ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, title, description, notes, url)
    VALUES ('delete', old.id, old.title, old.description, old.notes, old.url);
END;

CREATE TRIGGER IF NOT EXISTS bookmarks_fts_update AFTER UPDATE ON bookmarks BEGIN
    INSERT INTO bookmarks_fts(bookmarks_fts, rowid, title, description, notes, url)
    VALUES ('delete', old.id, old.title, old.description, old.notes, old.url);
    INSERT INTO bookmarks_fts(rowid, title, description, notes, url)
    VALUES (new.id, new.title, new.description, new.notes, new.url);
END;

CREATE TABLE IF NOT EXISTS capture_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bookmark_id INTEGER NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
    -- kind 含 'preview'（预览图任务，对齐上游 load_preview_image 后台任务）。
    kind TEXT NOT NULL CHECK (kind IN ('metadata', 'snapshot', 'archive_lookup', 'preview')),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'success', 'failed')),
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS audit_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER REFERENCES users(id) ON DELETE SET NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- 标签大小写不敏感唯一：插入/更新撞名（忽略大小写）时中止
-- （对齐 linkding Tag.name iexact 语义；SQLite 表级 UNIQUE 无法改为 NOCASE，
--   且其自带索引不可 DROP，故采用触发器保证唯一性）。
CREATE TRIGGER IF NOT EXISTS trg_tags_nocase_insert
BEFORE INSERT ON tags
WHEN EXISTS (
    SELECT 1 FROM tags WHERE user_id = NEW.user_id AND name = NEW.name COLLATE NOCASE
)
BEGIN
    SELECT RAISE(ABORT, 'UNIQUE constraint failed: tags.user_id, tags.name');
END;

CREATE TRIGGER IF NOT EXISTS trg_tags_nocase_update
BEFORE UPDATE OF name ON tags
WHEN EXISTS (
    SELECT 1 FROM tags WHERE user_id = NEW.user_id AND name = NEW.name COLLATE NOCASE AND id != NEW.id
)
BEGIN
    SELECT RAISE(ABORT, 'UNIQUE constraint failed: tags.user_id, tags.name');
END;

CREATE INDEX IF NOT EXISTS idx_bookmarks_user ON bookmarks(user_id);
CREATE INDEX IF NOT EXISTS idx_bookmarks_user_unread ON bookmarks(user_id, is_unread);
CREATE INDEX IF NOT EXISTS idx_bookmarks_user_archived ON bookmarks(user_id, is_archived);
CREATE INDEX IF NOT EXISTS idx_bookmarks_user_shared ON bookmarks(user_id, is_shared);
CREATE INDEX IF NOT EXISTS idx_tags_user ON tags(user_id);
-- 标签查询加速索引（唯一性由触发器保证，这里不建唯一索引以免与触发器双重约束）。
CREATE INDEX IF NOT EXISTS idx_tags_user_name_nocase ON tags (user_id, name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_bookmark_tags_tag ON bookmark_tags(tag_id);
CREATE INDEX IF NOT EXISTS idx_bookmark_assets_bookmark ON bookmark_assets(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_capture_tasks_bookmark ON capture_tasks(bookmark_id);
CREATE INDEX IF NOT EXISTS idx_capture_tasks_status ON capture_tasks(status);
CREATE INDEX IF NOT EXISTS idx_audit_logs_user ON audit_logs(user_id);
