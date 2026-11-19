-- linklist PostgreSQL 初始表结构（与 SQLite 0001_init.sql 对齐）。
-- 初始版本将全部结构合并为单一迁移；此后发布版本起追加增量迁移。
-- 以 DO 块保证可重复执行（sqlx 迁移表不适用：本文件由应用侧 raw_sql 执行）。

DO $$
BEGIN
    CREATE TABLE IF NOT EXISTS users (
        id BIGSERIAL PRIMARY KEY,
        username TEXT NOT NULL UNIQUE,
        password_hash TEXT NOT NULL,
        is_admin BOOLEAN NOT NULL DEFAULT FALSE,
        is_active BOOLEAN NOT NULL DEFAULT TRUE,
        date_joined TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS user_profiles (
        user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        theme TEXT NOT NULL DEFAULT 'auto',
        items_per_page INTEGER NOT NULL DEFAULT 30 CHECK (items_per_page >= 10),
        tag_grouping TEXT NOT NULL DEFAULT 'alphabetical',
        enable_favicons BOOLEAN NOT NULL DEFAULT TRUE,
        enable_preview_images BOOLEAN NOT NULL DEFAULT FALSE,
        enable_automatic_snapshots BOOLEAN NOT NULL DEFAULT FALSE,
        enable_sharing BOOLEAN NOT NULL DEFAULT FALSE,
        enable_public_sharing BOOLEAN NOT NULL DEFAULT FALSE,
        auto_tagging_rules TEXT NOT NULL DEFAULT '',
        custom_css TEXT NOT NULL DEFAULT '',
        search_preferences TEXT NOT NULL DEFAULT '{}',
        bookmark_date_display TEXT NOT NULL DEFAULT 'relative',
        bookmark_description_display TEXT NOT NULL DEFAULT 'inline',
        bookmark_description_max_lines INTEGER NOT NULL DEFAULT 5,
        display_url BOOLEAN NOT NULL DEFAULT TRUE,
        permanent_notes BOOLEAN NOT NULL DEFAULT FALSE,
        display_view_bookmark_action BOOLEAN NOT NULL DEFAULT TRUE,
        display_edit_bookmark_action BOOLEAN NOT NULL DEFAULT TRUE,
        display_archive_bookmark_action BOOLEAN NOT NULL DEFAULT TRUE,
        display_remove_bookmark_action BOOLEAN NOT NULL DEFAULT TRUE,
        bookmark_link_target TEXT NOT NULL DEFAULT '_blank',
        sticky_pagination BOOLEAN NOT NULL DEFAULT FALSE,
        collapse_side_panel BOOLEAN NOT NULL DEFAULT FALSE,
        hide_bundles BOOLEAN NOT NULL DEFAULT FALSE,
        tag_search TEXT NOT NULL DEFAULT 'strict',
        legacy_search BOOLEAN NOT NULL DEFAULT FALSE,
        web_archive_integration TEXT NOT NULL DEFAULT 'disabled',
        default_mark_unread BOOLEAN NOT NULL DEFAULT FALSE,
        default_mark_shared BOOLEAN NOT NULL DEFAULT TRUE
    );

    CREATE TABLE IF NOT EXISTS api_tokens (
        id BIGSERIAL PRIMARY KEY,
        user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name TEXT NOT NULL DEFAULT '',
        token_hash TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL,
        last_used_at TEXT,
        revoked_at TEXT,
        expires_at TEXT
    );

    CREATE TABLE IF NOT EXISTS feed_tokens (
        user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
        key TEXT NOT NULL UNIQUE
    );

    CREATE TABLE IF NOT EXISTS bookmarks (
        id BIGSERIAL PRIMARY KEY,
        user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        url TEXT NOT NULL,
        url_hash TEXT NOT NULL,
        title TEXT NOT NULL DEFAULT '',
        description TEXT NOT NULL DEFAULT '',
        notes TEXT NOT NULL DEFAULT '',
        is_unread BOOLEAN NOT NULL DEFAULT FALSE,
        is_archived BOOLEAN NOT NULL DEFAULT FALSE,
        is_shared BOOLEAN NOT NULL DEFAULT FALSE,
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
        id BIGSERIAL PRIMARY KEY,
        user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        UNIQUE (user_id, name)
    );

    CREATE TABLE IF NOT EXISTS bookmark_tags (
        bookmark_id BIGINT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
        tag_id BIGINT NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
        PRIMARY KEY (bookmark_id, tag_id)
    );

    CREATE TABLE IF NOT EXISTS bookmark_assets (
        id BIGSERIAL PRIMARY KEY,
        bookmark_id BIGINT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
        asset_type TEXT NOT NULL CHECK (asset_type IN ('snapshot', 'upload')),
        file_path TEXT NOT NULL,
        file_size BIGINT NOT NULL DEFAULT 0,
        content_type TEXT NOT NULL DEFAULT 'text/html',
        display_name TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL DEFAULT 'complete' CHECK (status IN ('pending', 'complete', 'failure')),
        date_created TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS bundles (
        id BIGSERIAL PRIMARY KEY,
        user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        name TEXT NOT NULL,
        search_json TEXT NOT NULL DEFAULT '{}',
        created_at TEXT NOT NULL,
        sort_order INTEGER NOT NULL DEFAULT 0
    );

    CREATE TABLE IF NOT EXISTS capture_tasks (
        id BIGSERIAL PRIMARY KEY,
        bookmark_id BIGINT NOT NULL REFERENCES bookmarks(id) ON DELETE CASCADE,
        kind TEXT NOT NULL CHECK (kind IN ('metadata', 'snapshot', 'archive_lookup', 'preview')),
        status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'running', 'success', 'failed')),
        attempts INTEGER NOT NULL DEFAULT 0,
        last_error TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS audit_logs (
        id BIGSERIAL PRIMARY KEY,
        user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
        action TEXT NOT NULL,
        target TEXT NOT NULL DEFAULT '',
        created_at TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_bookmarks_user ON bookmarks(user_id);
    CREATE INDEX IF NOT EXISTS idx_bookmarks_user_unread ON bookmarks(user_id, is_unread);
    CREATE INDEX IF NOT EXISTS idx_bookmarks_user_archived ON bookmarks(user_id, is_archived);
    CREATE INDEX IF NOT EXISTS idx_bookmarks_user_shared ON bookmarks(user_id, is_shared);
    CREATE INDEX IF NOT EXISTS idx_tags_user ON tags(user_id);
    CREATE INDEX IF NOT EXISTS idx_tags_user_name_lower ON tags (user_id, LOWER(name));
    CREATE INDEX IF NOT EXISTS idx_bookmark_tags_tag ON bookmark_tags(tag_id);
    CREATE INDEX IF NOT EXISTS idx_bookmark_assets_bookmark ON bookmark_assets(bookmark_id);
    CREATE INDEX IF NOT EXISTS idx_capture_tasks_bookmark ON capture_tasks(bookmark_id);
    CREATE INDEX IF NOT EXISTS idx_capture_tasks_status ON capture_tasks(status);
    CREATE INDEX IF NOT EXISTS idx_audit_logs_user ON audit_logs(user_id);
END $$;

-- 标签大小写不敏感唯一（对齐 SQLite 触发器与 linkding iexact 语义）。
CREATE OR REPLACE FUNCTION trg_tags_nocase_check() RETURNS trigger AS $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM tags
        WHERE user_id = NEW.user_id AND LOWER(name) = LOWER(NEW.name)
          AND (TG_OP = 'INSERT' OR id != NEW.id)
    ) THEN
        RAISE EXCEPTION 'UNIQUE constraint failed: tags.user_id, tags.name';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_tags_nocase_insert ON tags;
CREATE TRIGGER trg_tags_nocase_insert
BEFORE INSERT ON tags
FOR EACH ROW EXECUTE FUNCTION trg_tags_nocase_check();

DROP TRIGGER IF EXISTS trg_tags_nocase_update ON tags;
CREATE TRIGGER trg_tags_nocase_update
BEFORE UPDATE OF name ON tags
FOR EACH ROW EXECUTE FUNCTION trg_tags_nocase_check();
