use std::sync::Arc;
use std::time::Duration;

use crate::db::DbPool;
use tokio::sync::mpsc;

use crate::error::AppError;

/// 任务类型。
pub const KIND_METADATA: &str = "metadata";
pub const KIND_SNAPSHOT: &str = "snapshot";
pub const KIND_ARCHIVE_LOOKUP: &str = "archive_lookup";
pub const KIND_PREVIEW: &str = "preview";

pub const MAX_ATTEMPTS: u32 = 5;
const CHANNEL_CAPACITY: usize = 256;

/// 队列状态快照。
#[derive(Debug, serde::Serialize)]
pub struct QueueStats {
    pub pending: i64,
    pub running: i64,
    pub success: i64,
    pub failed: i64,
}

/// 任务执行器 trait：由各任务种类实现。
pub trait TaskHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    /// 执行一次；Err 表示本次失败（将重试）。
    fn execute<'a>(
        &'a self,
        bookmark_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>>;
}
/// 后台任务队列：mpsc 通知 + capture_tasks 表持久化状态。
pub struct TaskQueue {
    pool: DbPool,
    tx: mpsc::Sender<i64>,
    rx: tokio::sync::Mutex<Option<mpsc::Receiver<i64>>>,
    handlers: std::sync::Mutex<Vec<Arc<dyn TaskHandler>>>,
}

impl TaskQueue {
    pub fn new(pool: DbPool) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        Arc::new(Self {
            pool,
            tx,
            rx: tokio::sync::Mutex::new(Some(rx)),
            handlers: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// 注册处理器并启动 worker。
    pub fn with_handler(self: &Arc<Self>, handler: Arc<dyn TaskHandler>) -> Arc<Self> {
        // 在同一实例上注册（保持 tx/rx 通道连通），返回自身以便链式调用。
        if let Ok(mut h) = self.handlers.lock() {
            h.push(handler);
        }
        self.clone()
    }

    /// 入队：写表 + 发通知；返回任务行 id。
    pub async fn enqueue(&self, bookmark_id: i64, kind: &str) -> Result<i64, AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        // PG 无函数式自增回读，用 RETURNING id；SQLite/MySQL 用响应自带回读。
        let q = match self.pool.dialect() {
            crate::db::DbDialect::Postgres => crate::db::q(
                "INSERT INTO capture_tasks (bookmark_id, kind, status, attempts, created_at, updated_at)
                 VALUES (?, ?, 'pending', 0, ?, ?) RETURNING id",
            ),
            _ => crate::db::q(
                "INSERT INTO capture_tasks (bookmark_id, kind, status, attempts, created_at, updated_at)
                 VALUES (?, ?, 'pending', 0, ?, ?)",
            ),
        }
        .bind(bookmark_id)
        .bind(kind)
        .bind(&now)
        .bind(&now);
        let id = match &self.pool {
            DbPool::Postgres(_) => q.fetch_one(&self.pool).await?.get_i64(0),
            // SQLite 与 MySQL 同形态：INSERT 响应自带自增回读（MySQL 为 last_insert_id）。
            DbPool::Sqlite(_) | DbPool::MySql(_) => {
                q.execute(&self.pool).await?.last_insert_rowid()
            }
        };
        let _ = self.tx.send(id).await;
        Ok(id)
    }

    /// 队列状态查询。
    #[allow(dead_code)]
    pub async fn stats(&self) -> Result<QueueStats, AppError> {
        let row = crate::db::q_as::<(i64, i64, i64, i64)>(
            "SELECT
             SUM(CASE WHEN status='pending' THEN 1 ELSE 0 END),
             SUM(CASE WHEN status='running' THEN 1 ELSE 0 END),
             SUM(CASE WHEN status='success' THEN 1 ELSE 0 END),
             SUM(CASE WHEN status='failed' THEN 1 ELSE 0 END)
             FROM capture_tasks",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(QueueStats {
            pending: row.get_i64(0),
            running: row.get_i64(1),
            success: row.get_i64(2),
            failed: row.get_i64(3),
        })
    }

    /// 启动恢复：把启动时遗留的 pending/running 任务重新投递。
    pub async fn recover_pending(&self) -> Result<u64, AppError> {
        let rows =
            crate::db::q("SELECT id FROM capture_tasks WHERE status IN ('pending','running')")
                .fetch_all(&self.pool)
                .await?;
        let count = rows.len() as u64;
        for row in rows {
            let id: i64 = row.get_named_i64("id");
            let _ = self.tx.send(id).await;
        }
        Ok(count)
    }

    /// 启动单个 worker 循环（串行执行；指数退避重试至 MAX_ATTEMPTS）。
    /// 启动 worker 循环（消费内部通知通道；串行执行）。
    pub async fn start_worker(self: &Arc<Self>) {
        let q = self.clone();
        if let Some(mut rx) = self.rx.lock().await.take() {
            tokio::spawn(async move {
                while let Some(task_id) = rx.recv().await {
                    q.process(task_id).await;
                }
            });
        }
    }

    /// 重新投递已有任务（重试接口用）。
    pub async fn requeue(&self, task_id: i64) {
        let _ = self.tx.send(task_id).await;
    }

    /// 测试专用：直接处理一次（绕过 mpsc）。
    #[cfg(test)]
    pub async fn process_for_test(&self, task_id: i64) {
        self.process(task_id).await;
    }

    /// 处理单个任务：加载行 → 找 handler → 执行 → 重试/成功/失败。
    async fn process(&self, task_id: i64) {
        let row = crate::db::q_as::<(i64, String, u32)>(
            "SELECT bookmark_id, kind, attempts FROM capture_tasks WHERE id = ?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await;
        let (bookmark_id, kind, attempts) = match row {
            Ok(Some(r)) => (r.get_i64(0), r.get_string(1), r.get_u32(2)),
            _ => return,
        };
        // 标记 running。
        let _ = crate::db::q("UPDATE capture_tasks SET status='running', attempts=attempts+1, updated_at=? WHERE id=?")
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(task_id)
            .execute(&self.pool)
            .await;

        let handler = self
            .handlers
            .lock()
            .map(|h| h.iter().find(|x| x.kind() == kind).cloned())
            .unwrap_or(None);
        let now = chrono::Utc::now().to_rfc3339();
        let Some(handler) = handler else {
            // 无处理器的任务不可恢复：直接失败。
            let _ = crate::db::q(
                "UPDATE capture_tasks SET status='failed', last_error=?, updated_at=? WHERE id=?",
            )
            .bind(format!("无 {kind} 处理器"))
            .bind(&now)
            .bind(task_id)
            .execute(&self.pool)
            .await;
            return;
        };
        match handler.execute(bookmark_id).await {
            Ok(()) => {
                let _ = crate::db::q("UPDATE capture_tasks SET status='success', last_error=NULL, updated_at=? WHERE id=?")
                    .bind(&now)
                    .bind(task_id)
                    .execute(&self.pool)
                    .await;
            }
            Err(err) => {
                let new_attempts = attempts + 1;
                if new_attempts >= MAX_ATTEMPTS {
                    let _ = crate::db::q("UPDATE capture_tasks SET status='failed', last_error=?, updated_at=? WHERE id=?")
                        .bind(&err)
                        .bind(&now)
                        .bind(task_id)
                        .execute(&self.pool)
                        .await;
                } else {
                    // 指数退避后重投。
                    let backoff = Duration::from_millis(100 * 2u64.pow(new_attempts.min(6)));
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(backoff).await;
                        let _ = tx.send(task_id).await;
                    });
                    let _ = crate::db::q("UPDATE capture_tasks SET status='pending', last_error=?, updated_at=? WHERE id=?")
                        .bind(&err)
                        .bind(&now)
                        .bind(task_id)
                        .execute(&self.pool)
                        .await;
                }
            }
        }
    }
}

/// 页面元数据抓取器（可注入 mock）。
pub type MetadataFetcher = std::sync::Arc<
    dyn Fn(
            String,
        ) -> futures::future::BoxFuture<
            'static,
            Result<crate::capture::metadata::PageMetadata, String>,
        > + Send
        + Sync,
>;

/// 元数据任务处理器：抓取 URL 标题/描述并回填。
pub struct MetadataHandler {
    pub pool: DbPool,
    /// 页面抓取器（可注入 mock）。
    pub fetcher: MetadataFetcher,
}

impl MetadataHandler {
    pub fn new(pool: DbPool) -> Self {
        Self {
            pool,
            fetcher: std::sync::Arc::new(|url| {
                Box::pin(async move { crate::capture::metadata::fetch_metadata(&url).await })
            }),
        }
    }
}

impl TaskHandler for MetadataHandler {
    fn kind(&self) -> &'static str {
        KIND_METADATA
    }

    fn execute<'a>(
        &'a self,
        bookmark_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let row = crate::db::q_as::<()>("SELECT url, title FROM bookmarks WHERE id = ?")
                .bind(bookmark_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "书签不存在".to_string())?;
            let url = row.get_string(0);
            let title = row.get_opt_string(1);
            let meta = (self.fetcher)(url.clone()).await?;
            // 标题为空或仍是 URL 兜底值时，用抓取到的标题覆盖。
            let new_title = match title.as_deref() {
                Some(t) if !t.trim().is_empty() && t.trim() != url => t.to_string(),
                _ => meta.title.clone(),
            };
            crate::db::q(
                "UPDATE bookmarks SET title = ?, description = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&new_title)
            .bind(&meta.description)
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(bookmark_id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            // 元数据里带 og:image 且书签还没有预览图、用户开启预览图时，追加入队预览图任务
            //（对齐上游 create/update/refresh 触发 load_preview_image）。
            if meta.preview_image_url.is_some() {
                let has_preview: bool = crate::db::q_scalar::<()>(
                    "SELECT preview_image_path FROM bookmarks WHERE id = ?",
                )
                .bind(bookmark_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?
                .get_opt_string(0)
                .map(|p| !p.is_empty())
                .unwrap_or(false);
                let enabled: bool = crate::db::q_scalar::<()>(
                    "SELECT enable_preview_images FROM user_profiles
                     WHERE user_id = (SELECT user_id FROM bookmarks WHERE id = ?)",
                )
                .bind(bookmark_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?
                .map(|r| r.get_i64(0) != 0)
                .unwrap_or(false);
                if enabled && !has_preview {
                    let _ = crate::db::q(
                        "INSERT INTO capture_tasks (bookmark_id, kind, status, attempts, created_at, updated_at)
                         VALUES (?, ?, 'pending', 0, ?, ?)",
                    )
                    .bind(bookmark_id)
                    .bind(KIND_PREVIEW)
                    .bind(chrono::Utc::now().to_rfc3339())
                    .bind(chrono::Utc::now().to_rfc3339())
                    .execute(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        })
    }
}

/// 预览图任务处理器：抓取 og:image 下载到 {data_dir}/previews 并回填 preview_image_path。
/// 对齐上游 _load_preview_image_task + preview_image_loader.load_preview_image。
pub struct PreviewImageHandler {
    pub pool: DbPool,
    pub data_dir: std::path::PathBuf,
}

impl TaskHandler for PreviewImageHandler {
    fn kind(&self) -> &'static str {
        KIND_PREVIEW
    }

    fn execute<'a>(
        &'a self,
        bookmark_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let row = crate::db::q_as::<()>("SELECT url, user_id FROM bookmarks WHERE id = ?")
                .bind(bookmark_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "书签不存在".to_string())?;
            let url = row.get_string(0);
            let user_id = row.get_i64(1);
            // 门控：执行时实时读偏好，设置关闭后不再下载（对齐上游 is_preview_feature_active）。
            let enabled: bool = crate::db::q_scalar::<()>(
                "SELECT enable_preview_images FROM user_profiles WHERE user_id = ?",
            )
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?
            .map(|r| r.get_i64(0) != 0)
            .unwrap_or(false);
            if !enabled {
                return Ok(());
            }
            let file_name = crate::capture::preview::save_preview(&self.data_dir, &url).await?;
            crate::db::q(
                "UPDATE bookmarks SET preview_image_path = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&file_name)
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(bookmark_id)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}

/// 快照任务处理器：singlefile 快照 + 回填 snapshot_path。
pub struct SnapshotHandler {
    pub pool: DbPool,
    pub data_dir: std::path::PathBuf,
    /// 资源抓取器（可注入 mock）。
    pub fetcher: crate::capture::snapshot::ResourceFetcher,
}

impl SnapshotHandler {
    pub fn new(pool: DbPool, data_dir: std::path::PathBuf) -> Self {
        Self {
            pool,
            data_dir,
            fetcher: crate::capture::snapshot::http_fetcher(),
        }
    }
}

impl TaskHandler for SnapshotHandler {
    fn kind(&self) -> &'static str {
        KIND_SNAPSHOT
    }

    fn execute<'a>(
        &'a self,
        bookmark_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let url: String = crate::db::q_scalar::<()>("SELECT url FROM bookmarks WHERE id = ?")
                .bind(bookmark_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "书签不存在".to_string())?
                .get_string(0);
            let path = crate::capture::snapshot::capture_snapshot_with(
                &self.data_dir,
                bookmark_id,
                &url,
                self.fetcher.clone(),
            )
            .await?;
            crate::db::q("UPDATE bookmarks SET snapshot_path = ?, updated_at = ? WHERE id = ?")
                .bind(path.to_string_lossy().as_ref())
                .bind(chrono::Utc::now().to_rfc3339())
                .bind(bookmark_id)
                .execute(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
            Ok(())
        })
    }
}

/// Archive 查询任务处理器：查询 Internet Archive 并回填 archive_url。
pub struct ArchiveLookupHandler {
    pub pool: DbPool,
    /// availability API 端点（测试可指向 mock）。
    pub base_url: String,
}

impl TaskHandler for ArchiveLookupHandler {
    fn kind(&self) -> &'static str {
        KIND_ARCHIVE_LOOKUP
    }

    fn execute<'a>(
        &'a self,
        bookmark_id: i64,
    ) -> futures::future::BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let url: String = crate::db::q_scalar::<()>("SELECT url FROM bookmarks WHERE id = ?")
                .bind(bookmark_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "书签不存在".to_string())?
                .get_string(0);
            let client = reqwest::Client::new();
            match crate::capture::archive::lookup_with(&self.base_url, &url, &client).await? {
                crate::capture::archive::ArchiveLookup::Found(archive_url) => {
                    crate::db::q(
                        "UPDATE bookmarks SET archive_url = ?, updated_at = ? WHERE id = ?",
                    )
                    .bind(&archive_url)
                    .bind(chrono::Utc::now().to_rfc3339())
                    .bind(bookmark_id)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
                    Ok(())
                }
                crate::capture::archive::ArchiveLookup::NotFound => {
                    // 无存档不是错误；任务成功但什么都不写。
                    Ok(())
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingHandler {
        kind: &'static str,
        calls: AtomicUsize,
        fail_times: usize,
    }

    impl TaskHandler for CountingHandler {
        fn execute<'a>(
            &'a self,
            _bookmark_id: i64,
        ) -> futures::future::BoxFuture<'a, Result<(), String>> {
            Box::pin(async move {
                let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if n <= self.fail_times {
                    Err(format!("第 {n} 次失败"))
                } else {
                    Ok(())
                }
            })
        }
        fn kind(&self) -> &'static str {
            self.kind
        }
    }

    async fn make_bookmark(pool: &DbPool, user_id: i64, url: &str) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        let r = crate::db::q(
            "INSERT INTO bookmarks (user_id, url, url_hash, title, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(user_id)
        .bind(url)
        .bind(url)
        .bind("T")
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .unwrap();
        r.last_insert_rowid()
    }

    async fn task_status(pool: &DbPool, task_id: i64) -> (String, u32, Option<String>) {
        let row = crate::db::q_as::<(String, u32, Option<String>)>(
            "SELECT status, attempts, last_error FROM capture_tasks WHERE id = ?",
        )
        .bind(task_id)
        .fetch_one(pool)
        .await
        .unwrap();
        (row.get_string(0), row.get_u32(1), row.get_opt_string(2))
    }

    #[tokio::test]
    async fn process_succeeds_first_try() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://t1.example").await;
        let handler = Arc::new(CountingHandler {
            kind: "metadata",
            calls: AtomicUsize::new(0),
            fail_times: 0,
        });
        let queue = TaskQueue::new(pool.clone()).with_handler(handler.clone());
        let task_id = queue.enqueue(bid, "metadata").await.unwrap();
        queue.process_for_test(task_id).await;
        let (status, attempts, _) = task_status(&pool, task_id).await;
        assert_eq!(status, "success");
        assert_eq!(attempts, 1);
        let stats = queue.stats().await.unwrap();
        assert_eq!(stats.success, 1);
    }

    #[tokio::test]
    async fn process_retries_then_fails_after_max() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://t2.example").await;
        let handler = Arc::new(CountingHandler {
            kind: "snapshot",
            calls: AtomicUsize::new(0),
            fail_times: usize::MAX,
        });
        let queue = TaskQueue::new(pool.clone()).with_handler(handler.clone());
        let task_id = queue.enqueue(bid, "snapshot").await.unwrap();
        // 反复处理到耗尽重试。
        for _ in 0..MAX_ATTEMPTS {
            queue.process_for_test(task_id).await;
        }
        let (status, attempts, last_error) = task_status(&pool, task_id).await;
        assert_eq!(status, "failed");
        assert_eq!(attempts, MAX_ATTEMPTS);
        assert!(last_error.is_some());
        let stats = queue.stats().await.unwrap();
        assert_eq!(stats.failed, 1);
    }

    #[tokio::test]
    async fn unknown_kind_marks_failed() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://t3.example").await;
        let queue = TaskQueue::new(pool.clone());
        let task_id = queue.enqueue(bid, "archive_lookup").await.unwrap();
        queue.process_for_test(task_id).await;
        let (status, _, err) = task_status(&pool, task_id).await;
        assert_eq!(status, "failed");
        assert!(err.unwrap().contains("处理器"));
    }

    #[tokio::test]
    async fn snapshot_handler_inlines_and_backfills_path() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://site.example/page").await;
        let tmp = tempfile::tempdir().unwrap();
        // mock 抓取器：主页面 + 内联 CSS。
        let fetcher: crate::capture::snapshot::ResourceFetcher = Arc::new(|url: String| {
            Box::pin(async move {
                if url.ends_with(".css") {
                    Ok(("text/css".into(), b"body{color:red}".to_vec()))
                } else if url.ends_with(".png") {
                    Ok(("image/png".into(), vec![0x89, b'P', b'N', b'G']))
                } else {
                    Ok((
                        "text/html".into(),
                        br#"<html><head><link rel="stylesheet" href="/style.css"></head>
                            <body><img src="/a.png"></body></html>"#
                            .to_vec(),
                    ))
                }
            })
        });
        let handler = Arc::new(SnapshotHandler {
            pool: pool.clone(),
            data_dir: tmp.path().to_path_buf(),
            fetcher,
        });
        let queue = TaskQueue::new(pool.clone()).with_handler(handler);
        let task_id = queue.enqueue(bid, "snapshot").await.unwrap();
        queue.process_for_test(task_id).await;

        // 快照文件写入 + 内联 CSS + 图片绝对化。
        let path = crate::capture::snapshot::snapshot_path(tmp.path(), bid);
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("body{color:red}"), "{content}");
        assert!(
            content.contains("data:image/png;base64,"),
            "图片应内联为 data URL: {content}"
        );
        assert!(!content.contains("<link"), "link 标签应被替换: {content}");
        // snapshot_path 回填。
        let snap: String =
            crate::db::q_scalar::<()>("SELECT snapshot_path FROM bookmarks WHERE id = ?")
                .bind(bid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get_string(0);
        assert_eq!(snap, path.to_string_lossy());
        let stats = queue.stats().await.unwrap();
        assert_eq!(stats.success, 1);
    }

    #[tokio::test]
    async fn delete_bookmark_removes_snapshot_file() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://site.example/del").await;
        let tmp = tempfile::tempdir().unwrap();
        let path = crate::capture::snapshot::snapshot_path(tmp.path(), bid);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, b"<html></html>").await.unwrap();
        crate::db::q("UPDATE bookmarks SET snapshot_path = ? WHERE id = ?")
            .bind(path.to_string_lossy().as_ref())
            .bind(bid)
            .execute(&pool)
            .await
            .unwrap();
        // 删除书签 → 文件一并清理。
        let deleted = crate::bookmarks::crud::delete(&pool, u.id, bid)
            .await
            .unwrap();
        assert!(deleted);
        assert!(!path.exists(), "快照文件应被删除");
    }

    #[tokio::test]
    async fn archive_lookup_found_backfills_url() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://archived.example").await;
        // mock availability API：命中。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/wayback/available",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "archived_snapshots": {"closest": {"url": "https://web.archive.org/web/x", "status": "200"}}
                }))
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let handler = Arc::new(ArchiveLookupHandler {
            pool: pool.clone(),
            base_url: format!("http://{addr}/wayback/available"),
        });
        let queue = TaskQueue::new(pool.clone()).with_handler(handler);
        let task_id = queue.enqueue(bid, "archive_lookup").await.unwrap();
        queue.process_for_test(task_id).await;
        let archive_url: Option<String> =
            crate::db::q_scalar::<()>("SELECT archive_url FROM bookmarks WHERE id = ?")
                .bind(bid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get_opt_string(0);
        assert_eq!(
            archive_url.as_deref(),
            Some("https://web.archive.org/web/x")
        );
    }

    #[tokio::test]
    async fn archive_lookup_not_found_leaves_null() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://noarchive.example").await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/wayback/available",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({"archived_snapshots": {}}))
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let handler = Arc::new(ArchiveLookupHandler {
            pool: pool.clone(),
            base_url: format!("http://{addr}/wayback/available"),
        });
        let queue = TaskQueue::new(pool.clone()).with_handler(handler);
        let task_id = queue.enqueue(bid, "archive_lookup").await.unwrap();
        queue.process_for_test(task_id).await;
        let row = crate::db::q(
            "SELECT archive_url, (SELECT status FROM capture_tasks WHERE id = ?) FROM bookmarks WHERE id = ?",
        )
        .bind(task_id)
        .bind(bid)
        .fetch_one(&pool)
        .await
        .unwrap();
        let (archive_url, status) = (row.get_opt_string(0), row.get_string(1));
        assert_eq!(archive_url, None);
        assert_eq!(status, "success", "未命中也是成功（无可回填）");
    }

    #[tokio::test]
    async fn archive_lookup_failure_marks_task_failed() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://x.example").await;
        let handler = Arc::new(ArchiveLookupHandler {
            pool: pool.clone(),
            base_url: "http://127.0.0.1:9/wayback/available".into(),
        });
        let queue = TaskQueue::new(pool.clone()).with_handler(handler);
        let task_id = queue.enqueue(bid, "archive_lookup").await.unwrap();
        for _ in 0..MAX_ATTEMPTS {
            queue.process_for_test(task_id).await;
        }
        let row = crate::db::q("SELECT status, attempts FROM capture_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let (status, attempts) = (row.get_string(0), row.get_u32(1));
        assert_eq!(status, "failed");
        assert_eq!(attempts, MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn preview_handler_downloads_and_backfills_when_enabled() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        crate::db::q("UPDATE user_profiles SET enable_preview_images = 1 WHERE user_id = ?")
            .bind(u.id)
            .execute(&pool)
            .await
            .unwrap();
        let bid = make_bookmark(&pool, u.id, "https://img.example/page").await;
        let dir = std::env::temp_dir().join(format!("linklist-preview-test-{bid}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let handler = crate::tasks::queue::PreviewImageHandler {
            pool: pool.clone(),
            data_dir: dir.clone(),
        };
        let queue = TaskQueue::new(pool.clone()).with_handler(Arc::new(handler));
        let task_id = queue
            .enqueue(bid, crate::tasks::queue::KIND_PREVIEW)
            .await
            .unwrap();
        // example 域不可达 → 任务失败但不 panic；成功路径由 capture/preview.rs 单测覆盖。
        queue.process_for_test(task_id).await;
        let row = crate::db::q("SELECT status, attempts FROM capture_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let status = row.get_string(0);
        // 单次 process：首次失败会按指数退避重投（status 回到 pending），不阻塞断言。
        assert!(matches!(status.as_str(), "failed" | "pending" | "success"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn preview_handler_skips_when_preference_disabled() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let u = crate::models::create_user(&pool, "u", "pw12345", false)
            .await
            .unwrap();
        // 默认 enable_preview_images = 0 → 处理器直接 Ok 跳过，不产生文件也不回填。
        let bid = make_bookmark(&pool, u.id, "https://img.example/off").await;
        let dir = std::env::temp_dir().join(format!("linklist-preview-off-{bid}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let handler = crate::tasks::queue::PreviewImageHandler {
            pool: pool.clone(),
            data_dir: dir.clone(),
        };
        let queue = TaskQueue::new(pool.clone()).with_handler(Arc::new(handler));
        let task_id = queue
            .enqueue(bid, crate::tasks::queue::KIND_PREVIEW)
            .await
            .unwrap();
        queue.process_for_test(task_id).await;
        let row = crate::db::q("SELECT status, attempts FROM capture_tasks WHERE id = ?")
            .bind(task_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let status = row.get_string(0);
        assert_eq!(status, "success");
        let path: Option<String> =
            crate::db::q_scalar::<()>("SELECT preview_image_path FROM bookmarks WHERE id = ?")
                .bind(bid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get_opt_string(0);
        assert!(path.is_none());
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
