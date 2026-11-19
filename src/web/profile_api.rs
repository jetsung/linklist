use crate::db::DbPool;
use axum::Json;
use axum::extract::State;

use crate::bookmarks::auto_tag::{self, AutoTagRule};
use crate::error::AppError;
use crate::web::auth_api::{AppState, AuthUser};

/// `GET /api/v1/profile/auto-tagging-rules`：读取自动打标规则。
pub async fn get_rules(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let rules = auto_tag::get_rules(&state.pool, auth.0.id).await?;
    Ok(Json(
        serde_json::to_value(&rules).map_err(AppError::internal)?,
    ))
}

/// `PUT /api/v1/profile/auto-tagging-rules`：整体替换自动打标规则。
pub async fn set_rules(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(rules): Json<Vec<AutoTagRule>>,
) -> Result<Json<serde_json::Value>, AppError> {
    auto_tag::set_rules(&state.pool, auth.0.id, &rules).await?;
    Ok(Json(serde_json::json!({"ok": true, "count": rules.len()})))
}

/// `POST /api/v1/profile/feed-key`：生成（或复用）当前用户的 RSS 订阅 key。
pub async fn create_feed_key(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let existing: Option<String> =
        crate::db::q_scalar::<()>("SELECT key FROM feed_tokens WHERE user_id = ?")
            .bind(auth.0.id)
            .fetch_optional(&state.pool)
            .await?
            .and_then(|r| r.get_opt_string(0));
    if let Some(key) = existing {
        return Ok(Json(serde_json::json!({"key": key})));
    }
    let key = crate::auth::token::generate_feed_key();
    crate::db::q("INSERT INTO feed_tokens (user_id, key) VALUES (?, ?)")
        .bind(auth.0.id)
        .bind(&key)
        .execute(&state.pool)
        .await?;
    Ok(Json(serde_json::json!({"key": key})))
}

/// 显示偏好（对应 user_profiles 可编辑列）。
#[derive(serde::Deserialize, Debug)]
pub struct DisplayPreferences {
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub items_per_page: Option<i64>,
    #[serde(default)]
    pub tag_grouping: Option<String>,
    #[serde(default)]
    pub enable_favicons: Option<bool>,
    #[serde(default)]
    pub enable_preview_images: Option<bool>,
    #[serde(default)]
    pub enable_automatic_snapshots: Option<bool>,
    #[serde(default)]
    pub enable_sharing: Option<bool>,
    #[serde(default)]
    pub enable_public_sharing: Option<bool>,
    #[serde(default)]
    pub custom_css: Option<String>,
    #[serde(default)]
    pub bookmark_date_display: Option<String>,
    #[serde(default)]
    pub bookmark_description_display: Option<String>,
    #[serde(default)]
    pub bookmark_description_max_lines: Option<i64>,
    #[serde(default)]
    pub display_url: Option<bool>,
    #[serde(default)]
    pub permanent_notes: Option<bool>,
    #[serde(default)]
    pub display_view_bookmark_action: Option<bool>,
    #[serde(default)]
    pub display_edit_bookmark_action: Option<bool>,
    #[serde(default)]
    pub display_archive_bookmark_action: Option<bool>,
    #[serde(default)]
    pub display_remove_bookmark_action: Option<bool>,
    #[serde(default)]
    pub bookmark_link_target: Option<String>,
    #[serde(default)]
    pub sticky_pagination: Option<bool>,
    #[serde(default)]
    pub collapse_side_panel: Option<bool>,
    #[serde(default)]
    pub hide_bundles: Option<bool>,
    #[serde(default)]
    pub tag_search: Option<String>,
    #[serde(default)]
    pub legacy_search: Option<bool>,
    #[serde(default)]
    pub web_archive_integration: Option<String>,
    #[serde(default)]
    pub default_mark_unread: Option<bool>,
    #[serde(default)]
    pub default_mark_shared: Option<bool>,
}

/// 生效资料（对齐上游 LinkdingMiddleware 的 request.user_profile）：
/// 登录用户 → 本人 user_profiles；匿名 → settings.guest_profile_user 指向用户的资料，
/// 未设置或指向失效时 → 标准资料（standard_profile，对齐上游 enable_favicons=true）。
pub struct EffectiveProfile {
    pub user_id: Option<i64>,
    pub theme: String,
    pub items_per_page: i64,
    pub custom_css: String,
    pub enable_favicons: bool,
    pub enable_preview_images: bool,
    pub bookmark_date_display: String,
    pub bookmark_description_display: String,
    pub display_url: bool,
}

/// 解析生效资料：viewer_id 为 None（匿名）时按全局设置取访客资料。
pub async fn effective_profile(pool: &DbPool, viewer_id: Option<i64>) -> EffectiveProfile {
    // 标准资料（对齐上游 standard_profile）。
    let standard = EffectiveProfile {
        user_id: None,
        theme: "auto".into(),
        items_per_page: 30,
        custom_css: String::new(),
        enable_favicons: true,
        enable_preview_images: false,
        bookmark_date_display: "relative".into(),
        bookmark_description_display: "inline".into(),
        display_url: true,
    };
    let target_id = match viewer_id {
        Some(id) => id,
        None => {
            let raw = setting_value(pool, "guest_profile_user", "").await;
            let parsed = raw.trim().parse::<i64>().ok();
            match parsed {
                Some(id) if id > 0 => id,
                _ => return standard,
            }
        }
    };
    crate::db::q(
        "SELECT user_id, theme, items_per_page, custom_css, enable_favicons,
                enable_preview_images, bookmark_date_display, bookmark_description_display,
                display_url
         FROM user_profiles WHERE user_id = ?",
    )
    .bind(target_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .map(|r| EffectiveProfile {
        user_id: Some(r.get_i64(0)),
        theme: r.get_string(1),
        items_per_page: r.get_i64(2).clamp(1, 200),
        custom_css: r.get_string(3),
        enable_favicons: r.get_i64(4) != 0,
        enable_preview_images: r.get_i64(5) != 0,
        bookmark_date_display: r.get_string(6),
        bookmark_description_display: r.get_string(7),
        display_url: r.get_i64(8) != 0,
    })
    .unwrap_or(standard)
}

async fn setting_value(pool: &DbPool, key: &str, default: &str) -> String {
    crate::db::q_scalar::<String>("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .map(|r| r.get_named_string("value"))
        .unwrap_or_else(|| default.to_string())
}

/// `GET /api/v1/global-settings`：匿名可读的站点级设置（首页落地页跳转、链接预取用）。
pub async fn public_global_settings(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let landing_page = setting_value(&state.pool, "landing_page", "login").await;
    let enable_link_prefetch = setting_value(&state.pool, "enable_link_prefetch", "false").await;
    Ok(Json(serde_json::json!({
        "landing_page": if landing_page == "shared_bookmarks" { "shared_bookmarks" } else { "login" },
        "enable_link_prefetch": enable_link_prefetch == "true",
    })))
}

/// `GET /api/v1/profile`：读取显示偏好（无行则返回默认值）。
pub async fn get_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let row = crate::db::q_as::<()>(
        "SELECT theme, items_per_page, tag_grouping, enable_favicons, enable_preview_images,
                enable_automatic_snapshots, enable_sharing, enable_public_sharing, custom_css,
                bookmark_date_display, bookmark_description_display, bookmark_description_max_lines,
                display_url, permanent_notes, display_view_bookmark_action, display_edit_bookmark_action,
                display_archive_bookmark_action, display_remove_bookmark_action, bookmark_link_target,
                sticky_pagination, collapse_side_panel, hide_bundles, tag_search, legacy_search,
                web_archive_integration, default_mark_unread, default_mark_shared
         FROM user_profiles WHERE user_id = ?",
    )
    .bind(auth.0.id)
    .fetch_optional(&state.pool)
    .await?;
    let v = match row {
        Some(r) => serde_json::json!({
            "theme": r.get_named_string("theme"),
            "items_per_page": r.get_named_i64("items_per_page"),
            "tag_grouping": r.get_named_string("tag_grouping"),
            "enable_favicons": r.get_named_bool("enable_favicons"),
            "enable_preview_images": r.get_named_bool("enable_preview_images"),
            "enable_automatic_snapshots": r.get_named_bool("enable_automatic_snapshots"),
            "enable_sharing": r.get_named_bool("enable_sharing"),
            "enable_public_sharing": r.get_named_bool("enable_public_sharing"),
            "custom_css": r.get_named_string("custom_css"),
            "bookmark_date_display": r.get_named_string("bookmark_date_display"),
            "bookmark_description_display": r.get_named_string("bookmark_description_display"),
            "bookmark_description_max_lines": r.get_named_i64("bookmark_description_max_lines"),
            "display_url": r.get_named_bool("display_url"),
            "permanent_notes": r.get_named_bool("permanent_notes"),
            "display_view_bookmark_action": r.get_named_bool("display_view_bookmark_action"),
            "display_edit_bookmark_action": r.get_named_bool("display_edit_bookmark_action"),
            "display_archive_bookmark_action": r.get_named_bool("display_archive_bookmark_action"),
            "display_remove_bookmark_action": r.get_named_bool("display_remove_bookmark_action"),
            "bookmark_link_target": r.get_named_string("bookmark_link_target"),
            "sticky_pagination": r.get_named_bool("sticky_pagination"),
            "collapse_side_panel": r.get_named_bool("collapse_side_panel"),
            "hide_bundles": r.get_named_bool("hide_bundles"),
            "tag_search": r.get_named_string("tag_search"),
            "legacy_search": r.get_named_bool("legacy_search"),
            "web_archive_integration": r.get_named_string("web_archive_integration"),
            "default_mark_unread": r.get_named_bool("default_mark_unread"),
            "default_mark_shared": r.get_named_bool("default_mark_shared"),
        }),
        None => serde_json::json!({
            "theme": "auto", "items_per_page": 30, "tag_grouping": "alphabetical",
            "enable_favicons": true, "enable_preview_images": false,
            "enable_automatic_snapshots": false, "enable_sharing": false,
            "enable_public_sharing": false, "custom_css": "",
            "bookmark_date_display": "relative", "bookmark_description_display": "inline",
            "bookmark_description_max_lines": 5, "display_url": true, "permanent_notes": false,
            "display_view_bookmark_action": true, "display_edit_bookmark_action": true,
            "display_archive_bookmark_action": true, "display_remove_bookmark_action": true,
            "bookmark_link_target": "_blank", "sticky_pagination": false,
            "collapse_side_panel": false, "hide_bundles": false, "tag_search": "strict",
            "legacy_search": false, "web_archive_integration": "disabled",
            "default_mark_unread": false, "default_mark_shared": true,
        }),
    };
    Ok(Json(v))
}

/// `PUT /api/v1/profile`：更新显示偏好（部分更新；影响抓取与界面行为）。
pub async fn set_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<DisplayPreferences>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(t) = &req.theme
        && !["auto", "light", "dark"].contains(&t.as_str())
    {
        return Err(AppError::BadRequest(format!("无效主题: {t}")));
    }
    if let Some(n) = req.items_per_page
        && !(10..=200).contains(&n)
    {
        return Err(AppError::BadRequest("每页条数须在 10-200".into()));
    }
    if let Some(g) = &req.tag_grouping
        && !["alphabetical", "disabled"].contains(&g.as_str())
    {
        return Err(AppError::BadRequest(format!("无效标签分组: {g}")));
    }
    if let Some(v) = &req.bookmark_date_display
        && !["relative", "absolute", "hidden"].contains(&v.as_str())
    {
        return Err(AppError::BadRequest(format!("无效日期显示: {v}")));
    }
    if let Some(v) = &req.bookmark_description_display
        && !["inline", "separate"].contains(&v.as_str())
    {
        return Err(AppError::BadRequest(format!("无效描述显示: {v}")));
    }
    if let Some(v) = req.bookmark_description_max_lines
        && !(0..=50).contains(&v)
    {
        return Err(AppError::BadRequest("描述行数须在 0-50".into()));
    }
    if let Some(v) = &req.bookmark_link_target
        && !["_blank", "_self"].contains(&v.as_str())
    {
        return Err(AppError::BadRequest(format!("无效链接打开方式: {v}")));
    }
    if let Some(v) = &req.tag_search
        && !["strict", "lax"].contains(&v.as_str())
    {
        return Err(AppError::BadRequest(format!("无效标签搜索模式: {v}")));
    }
    if let Some(v) = &req.web_archive_integration
        && !["disabled", "enabled"].contains(&v.as_str())
    {
        return Err(AppError::BadRequest(format!("无效存档集成设置: {v}")));
    }
    // 确保行存在。
    crate::db::q(crate::db::sql_insert_ignore(
        state.pool.dialect(),
        "user_profiles (user_id)",
        "?",
    ))
    .bind(auth.0.id)
    .execute(&state.pool)
    .await?;
    crate::db::q(
        "UPDATE user_profiles SET
            theme = COALESCE(?, theme),
            items_per_page = COALESCE(?, items_per_page),
            tag_grouping = COALESCE(?, tag_grouping),
            enable_favicons = COALESCE(?, enable_favicons),
            enable_preview_images = COALESCE(?, enable_preview_images),
            enable_automatic_snapshots = COALESCE(?, enable_automatic_snapshots),
            enable_sharing = COALESCE(?, enable_sharing),
            enable_public_sharing = COALESCE(?, enable_public_sharing),
            custom_css = COALESCE(?, custom_css),
            bookmark_date_display = COALESCE(?, bookmark_date_display),
            bookmark_description_display = COALESCE(?, bookmark_description_display),
            bookmark_description_max_lines = COALESCE(?, bookmark_description_max_lines),
            display_url = COALESCE(?, display_url),
            permanent_notes = COALESCE(?, permanent_notes),
            display_view_bookmark_action = COALESCE(?, display_view_bookmark_action),
            display_edit_bookmark_action = COALESCE(?, display_edit_bookmark_action),
            display_archive_bookmark_action = COALESCE(?, display_archive_bookmark_action),
            display_remove_bookmark_action = COALESCE(?, display_remove_bookmark_action),
            bookmark_link_target = COALESCE(?, bookmark_link_target),
            sticky_pagination = COALESCE(?, sticky_pagination),
            collapse_side_panel = COALESCE(?, collapse_side_panel),
            hide_bundles = COALESCE(?, hide_bundles),
            tag_search = COALESCE(?, tag_search),
            legacy_search = COALESCE(?, legacy_search),
            web_archive_integration = COALESCE(?, web_archive_integration),
            default_mark_unread = COALESCE(?, default_mark_unread),
            default_mark_shared = COALESCE(?, default_mark_shared)
         WHERE user_id = ?",
    )
    .bind(req.theme)
    .bind(req.items_per_page)
    .bind(req.tag_grouping)
    .bind(req.enable_favicons.map(|b| b as i64))
    .bind(req.enable_preview_images.map(|b| b as i64))
    .bind(req.enable_automatic_snapshots.map(|b| b as i64))
    .bind(req.enable_sharing.map(|b| b as i64))
    .bind(req.enable_public_sharing.map(|b| b as i64))
    .bind(req.custom_css)
    .bind(req.bookmark_date_display)
    .bind(req.bookmark_description_display)
    .bind(req.bookmark_description_max_lines)
    .bind(req.display_url.map(|b| b as i64))
    .bind(req.permanent_notes.map(|b| b as i64))
    .bind(req.display_view_bookmark_action.map(|b| b as i64))
    .bind(req.display_edit_bookmark_action.map(|b| b as i64))
    .bind(req.display_archive_bookmark_action.map(|b| b as i64))
    .bind(req.display_remove_bookmark_action.map(|b| b as i64))
    .bind(req.bookmark_link_target)
    .bind(req.sticky_pagination.map(|b| b as i64))
    .bind(req.collapse_side_panel.map(|b| b as i64))
    .bind(req.hide_bundles.map(|b| b as i64))
    .bind(req.tag_search)
    .bind(req.legacy_search.map(|b| b as i64))
    .bind(req.web_archive_integration)
    .bind(req.default_mark_unread.map(|b| b as i64))
    .bind(req.default_mark_shared.map(|b| b as i64))
    .bind(auth.0.id)
    .execute(&state.pool)
    .await?;
    // 预览图开关 false→true 时补抓该用户所有无预览图书签
    //（对齐上游 schedule_bookmarks_without_previews）。
    if req.enable_preview_images == Some(true)
        && let Some(queue) = &state.task_queue
        && !state.config.disable_background_tasks
    {
        let ids: Vec<i64> = crate::db::q_scalar::<()>(
            "SELECT id FROM bookmarks
             WHERE user_id = ? AND (preview_image_path IS NULL OR preview_image_path = '')",
        )
        .bind(auth.0.id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.get_i64(0))
        .collect();
        for id in ids {
            if let Err(e) = queue.enqueue(id, crate::tasks::queue::KIND_PREVIEW).await {
                tracing::warn!("入队 preview 补抓任务失败 id={id}: {e}");
            }
        }
    }
    get_preferences(State(state), auth).await
}

/// 搜索偏好（对齐原版 user_profile.search_preferences）。
#[derive(serde::Deserialize, Debug)]
pub struct SearchPreferences {
    #[serde(default)]
    pub sort: Option<String>,
    #[serde(default)]
    pub shared: Option<String>,
    #[serde(default)]
    pub unread: Option<String>,
}

fn search_prefs_key(user_id: i64) -> String {
    format!("search_prefs:{user_id}")
}

/// `GET /api/v1/profile/search-preferences`：读取当前用户保存的搜索偏好（无则返回默认值）。
pub async fn get_search_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = search_prefs_key(auth.0.id);
    let raw: Option<String> = crate::db::q_scalar::<()>("SELECT value FROM settings WHERE key = ?")
        .bind(&key)
        .fetch_optional(&state.pool)
        .await?
        .map(|r| r.get_named_string("value"));
    let v = raw
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(
            || serde_json::json!({"sort": "added_desc", "shared": "off", "unread": "off"}),
        );
    Ok(Json(v))
}

/// `PUT /api/v1/profile/search-preferences`：保存当前用户搜索偏好（Save as default）。
pub async fn set_search_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(req): Json<SearchPreferences>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Some(s) = &req.sort
        && ![
            "added_asc",
            "added_desc",
            "modified_asc",
            "modified_desc",
            "title_asc",
            "title_desc",
        ]
        .contains(&s.as_str())
    {
        return Err(AppError::BadRequest(format!("无效排序: {s}")));
    }
    if let Some(s) = &req.shared
        && !["off", "yes", "no"].contains(&s.as_str())
    {
        return Err(AppError::BadRequest(format!("无效共享过滤: {s}")));
    }
    if let Some(s) = &req.unread
        && !["off", "yes", "no"].contains(&s.as_str())
    {
        return Err(AppError::BadRequest(format!("无效未读过滤: {s}")));
    }
    let key = search_prefs_key(auth.0.id);
    // 合并保存：未提供的字段沿用已存值。
    let existing: Option<String> =
        crate::db::q_scalar::<()>("SELECT value FROM settings WHERE key = ?")
            .bind(&key)
            .fetch_optional(&state.pool)
            .await?
            .map(|r| r.get_named_string("value"));
    let mut merged: serde_json::Value = existing
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(s) = req.sort {
        merged["sort"] = serde_json::Value::String(s);
    }
    if let Some(s) = req.shared {
        merged["shared"] = serde_json::Value::String(s);
    }
    if let Some(s) = req.unread {
        merged["unread"] = serde_json::Value::String(s);
    }
    let body = merged.to_string();
    crate::db::q(crate::db::sql_settings_upsert(state.pool.dialect()))
        .bind(&key)
        .bind(&body)
        .execute(&state.pool)
        .await?;
    Ok(Json(merged))
}

/// `POST /api/v1/profile/feed-key/regenerate`：轮换当前用户的 RSS 订阅 key。
pub async fn regenerate_feed_key(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let key = crate::auth::token::generate_feed_key();
    let updated = crate::db::q("UPDATE feed_tokens SET key = ? WHERE user_id = ?")
        .bind(&key)
        .bind(auth.0.id)
        .execute(&state.pool)
        .await?;
    if updated.rows_affected() == 0 {
        crate::db::q("INSERT INTO feed_tokens (user_id, key) VALUES (?, ?)")
            .bind(auth.0.id)
            .bind(&key)
            .execute(&state.pool)
            .await?;
    }
    Ok(Json(serde_json::json!({"key": key})))
}

/// `POST /api/v1/profile/refresh-favicons`：清空 favicon 缓存并重新抓取。
pub async fn refresh_favicons(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, AppError> {
    let _ = auth;
    crate::web::favicon::clear_cache();
    crate::web::favicon::clear_db(&state.pool).await;
    Ok(Json(serde_json::json!({"ok": true})))
}
