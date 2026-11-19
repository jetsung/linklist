use crate::db::DbPool;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::error::AppError;
use crate::web::auth_api::AppState;

/// favicon 代理：使用 Google favicon 服务（中国大陆可达的 gstatic.cn 优先）。
/// `GET /favicon/{host}` → 返回图片 PNG，失败回退 404（前端显示占位）。
///
/// 模块级共享客户端与内存缓存（进程内，TTL 24h）。
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("Mozilla/5.0 (compatible; linklist)")
        .build()
        .expect("favicon client")
});

type FaviconCache = Mutex<Option<HashMap<String, (Vec<u8>, Instant)>>>;

static CACHE: FaviconCache = Mutex::new(None);
const CACHE_TTL: Duration = Duration::from_secs(24 * 3600);

fn cache_get(host: &str) -> Option<Vec<u8>> {
    let map = CACHE.lock().ok()?;
    let map = map.as_ref()?;
    let (data, at) = map.get(host)?;
    if at.elapsed() < CACHE_TTL {
        Some(data.clone())
    } else {
        None
    }
}

fn cache_put(host: &str, data: Vec<u8>) {
    if let Ok(mut guard) = CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() > 4096 {
            map.clear();
        }
        map.insert(host.to_string(), (data, Instant::now()));
    }
}

/// 图标服务抓取链（参考 hugo-hive .deploy.sh 的图标源清单）：
/// google_cn（t3.gstatic.cn）→ google（t1.gstatic.com）→ yandex → cccyun。
fn icon_urls(host: &str) -> Vec<String> {
    vec![
        // google_cn
        format!(
            "https://t3.gstatic.cn/faviconV2?client=SOCIAL&type=FAVICON&fallback_opts=TYPE,SIZE,URL&url={host}&size=48"
        ),
        // google
        format!(
            "https://t1.gstatic.com/faviconV2?client=SOCIAL&type=FAVICON&fallback_opts=TYPE,SIZE,URL&url={host}&size=48"
        ),
        // yandex
        format!("https://favicon.yandex.net/favicon/{host}"),
        // cccyun
        format!("https://favicon.cccyun.cc/{host}"),
    ]
}

/// 依次尝试抓取图标，返回第一个成功的字节流。
async fn fetch_icon(host: &str) -> Option<Vec<u8>> {
    for url in icon_urls(host) {
        let Ok(resp) = CLIENT.get(&url).send().await else {
            continue;
        };
        if !resp.status().is_success() {
            continue;
        }
        let Ok(bytes) = resp.bytes().await else {
            continue;
        };
        if !bytes.is_empty() {
            return Some(bytes.to_vec());
        }
    }
    None
}

pub async fn favicon(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> Result<Response, AppError> {
    // 校验 host 形态，防止 SSRF/路径注入。
    let host = host.to_lowercase();
    let ok = !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !host.starts_with('.')
        && !host.ends_with('.');
    if !ok {
        return Err(AppError::BadRequest("无效域名".into()));
    }

    if let Some(data) = cache_get(&host) {
        return Ok(image_response(data));
    }

    // 先查 sqlite 缓存表（可选；无表则忽略错误）。
    if let Some(data) = db_lookup(&state, &host).await {
        cache_put(&host, data.clone());
        return Ok(image_response(data));
    }

    // 抓取链：google_cn → google → yandex → cccyun。
    if let Some(data) = fetch_icon(&host).await {
        cache_put(&host, data.clone());
        db_store(&state, &host, &data).await;
        return Ok(image_response(data));
    }
    Err(AppError::NotFound)
}

fn image_response(data: Vec<u8>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png".to_string()),
            (header::CACHE_CONTROL, "public, max-age=86400".to_string()),
        ],
        data,
    )
        .into_response()
}

async fn db_lookup(state: &AppState, host: &str) -> Option<Vec<u8>> {
    crate::db::q_scalar::<()>("SELECT data FROM favicons WHERE host = ?")
        .bind(host)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten()
        .map(|r| r.get_named_bytes("data"))
}

async fn db_store(state: &AppState, host: &str, data: &[u8]) {
    let _ = crate::db::q(crate::db::sql_favicon_upsert(state.pool.dialect()))
        .bind(host)
        .bind(data)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&state.pool)
        .await;
}

/// 清空 favicon 缓存（内存 + 数据库），下次请求时重新抓取。
pub fn clear_cache() {
    if let Ok(mut guard) = CACHE.lock() {
        *guard = None;
    }
}

pub async fn clear_db(pool: &DbPool) {
    let _ = crate::db::q("DELETE FROM favicons").execute(pool).await;
}

pub async fn ensure_table(pool: &DbPool) {
    let _ = crate::db::q(
        "CREATE TABLE IF NOT EXISTS favicons (
            host TEXT PRIMARY KEY,
            data BLOB NOT NULL,
            updated_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await;
}
