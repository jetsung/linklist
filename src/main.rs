mod auth;
mod bookmarks;
mod capture;
mod config;
mod db;
mod error;
mod models;
mod tasks;
mod web;

use std::path::Path;

use config::Config;

/// 命令行入口：无参 = 启动 Web 服务。
/// 子命令：run（默认）/ init / reset-password / version / help。
/// 退出码约定：成功 0，参数错误 2。
fn main() {
    let code = run();
    std::process::exit(code);
}

/// 输出完整用法（`help` / `--help` / `-h`）。
fn print_help() {
    println!(
        "linklist {} —— 自托管书签管理服务（linkding 的 Rust 重写版）

用法: linklist [子命令] [参数]

子命令:
  run                             启动 Web 服务（无参数时默认执行）
  init [--admin-password <密码>] [配置文件路径]
                                  初始化项目：生成配置文件（已存在不覆盖）→ 创建数据目录
                                  → 连接数据库并执行迁移；--admin-password 在库中无用户时
                                  预置管理员账号 admin（也可用环境变量
                                  LINKLIST_ADMIN_PASSWORD，两者都缺省则跳过预置）
  reset-password <username>       交互式重置指定用户的口令（不回显输入）
  version                         输出版本号（--version / -V 等价）
  help                            输出本帮助（--help / -h 等价）

数据库（环境变量 LINKLIST_DATABASE_URL，兼容通用别名 DATABASE_URL，前者优先）:
  连接串前缀决定后端：sqlite://（默认，留空时用 data_dir 下 linklist.db）/
  postgres://|postgresql:// / mysql://|mariadb://
  其余配置见 config.toml.example 与文档（docs/）。

退出码: 成功 0；参数错误/连接失败 2",
        env!("CARGO_PKG_VERSION")
    );
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        None | Some("run") => cmd_run(),
        Some("init") => cmd_init(&args[1..]),
        Some("reset-password") => cmd_reset_password(args.get(1).map(|s| s.as_str())),
        Some("version") | Some("--version") | Some("-V") => {
            println!("linklist {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("未知子命令: {other}");
            eprintln!("运行 `linklist help` 查看用法（子命令与退出码约定）");
            2
        }
    }
}

/// 启动 Web 服务（任务 2.x 中接入 axum 后替换为真实服务）。
fn cmd_run() -> i32 {
    let cfg = Config::load();
    // 注入 SSRF 内网主机白名单（LINKLIST_ALLOWED_INTERNAL_HOSTS）。
    crate::capture::ssrf::set_allowed_internal_hosts(cfg.allowed_internal_hosts.clone());
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_new(&cfg.log).unwrap_or_default())
        .init();
    println!(
        "linklist {} 启动于 {}",
        env!("CARGO_PKG_VERSION"),
        cfg.listen
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("创建 tokio runtime 失败");

    runtime.block_on(async move { run_server(cfg).await })
}

async fn run_server(cfg: Config) -> i32 {
    use std::sync::Arc;
    use tower_sessions_sqlx_store::SqliteStore;

    // 数据库：LINKLIST_DATABASE_URL 指向 postgres 时用 PostgreSQL，否则用 data_dir 下 SQLite。
    let db_path = std::path::Path::new(&cfg.data_dir).join("linklist.db");
    if let Some(parent) = db_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let sqlite_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = match crate::db::connect_any(&cfg.database_url, &sqlite_url).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("数据库连接失败: {e}");
            return 2;
        }
    };
    // favicon 兼容表仅 SQLite 需要（PG 走统一 favicons 表迁移，见 ensure_table）。
    if matches!(pool, crate::db::DbPool::Sqlite(_)) {
        crate::web::favicon::ensure_table(&pool).await;
    }

    // 会话存储（SQLite / PostgreSQL / MySQL 同库）。
    // 后台任务队列（metadata/snapshot/archive_lookup/preview）。
    let queue = crate::tasks::queue::TaskQueue::new(pool.clone())
        .with_handler(Arc::new(crate::tasks::queue::MetadataHandler::new(
            pool.clone(),
        )))
        .with_handler(Arc::new(crate::tasks::queue::SnapshotHandler::new(
            pool.clone(),
            cfg.data_dir.clone(),
        )))
        .with_handler(Arc::new(crate::tasks::queue::ArchiveLookupHandler {
            pool: pool.clone(),
            base_url: crate::capture::archive::DEFAULT_AVAILABILITY_URL.to_string(),
        }))
        .with_handler(Arc::new(crate::tasks::queue::PreviewImageHandler {
            pool: pool.clone(),
            data_dir: cfg.data_dir.clone(),
        }));
    queue.clone().start_worker().await;
    let state = crate::web::auth_api::AppState {
        pool: pool.clone(),
        login_limiter: Arc::new(crate::auth::rate_limit::LoginLimiter::new(5, 300)),
        config: cfg.clone(),
        task_queue: Some(queue.clone()),
    };
    let app = match &pool {
        crate::db::DbPool::Sqlite(_) => {
            let session_pool = match sqlx::pool::PoolOptions::<sqlx::Sqlite>::new()
                .max_connections(4)
                .connect(&sqlite_url)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("会话存储连接失败: {e}");
                    return 2;
                }
            };
            let session_store = SqliteStore::new(session_pool);
            if let Err(e) = session_store.migrate().await {
                eprintln!("会话表迁移失败: {e}");
            }
            crate::web::api_router(state)
                .layer(build_session_layer(session_store, cfg.session_ttl_secs))
        }
        crate::db::DbPool::Postgres(_) => {
            let session_pool = match sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
                .max_connections(4)
                .connect(&cfg.database_url)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("会话存储连接失败: {e}");
                    return 2;
                }
            };
            let session_store = tower_sessions_sqlx_store::PostgresStore::new(session_pool);
            if let Err(e) = session_store.migrate().await {
                eprintln!("会话表迁移失败: {e}");
            }
            crate::web::api_router(state)
                .layer(build_session_layer(session_store, cfg.session_ttl_secs))
        }
        // MySQL 会话存储与主库同库（tower-sessions-sqlx-store 的 MySqlStore），
        // 过期策略与其余后端一致（build_session_layer 统一配置）。
        crate::db::DbPool::MySql(_) => {
            let session_pool = match sqlx::pool::PoolOptions::<sqlx::MySql>::new()
                .max_connections(4)
                .connect(&cfg.database_url)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("会话存储连接失败: {e}");
                    return 2;
                }
            };
            // MySqlStore 默认在 tower_sessions schema（MySQL 中即独立数据库）建表，
            // 改为跟随连接串中的数据库，避免额外的 CREATE DATABASE 权限要求。
            let db_name = url::Url::parse(&cfg.database_url)
                .ok()
                .and_then(|u| {
                    u.path()
                        .trim_start_matches('/')
                        .split('?')
                        .next()
                        .map(|s| s.to_string())
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "linklist".to_string());
            let session_store = tower_sessions_sqlx_store::MySqlStore::new(session_pool)
                .with_schema_name(db_name)
                .expect("会话 schema 名无效");
            if let Err(e) = session_store.migrate().await {
                eprintln!("会话表迁移失败: {e}");
            }
            crate::web::api_router(state)
                .layer(build_session_layer(session_store, cfg.session_ttl_secs))
        }
    };

    let listener = match tokio::net::TcpListener::bind(&cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("监听 {} 失败: {e}", cfg.listen);
            return 2;
        }
    };
    println!("就绪: http://{}", cfg.listen);
    // 恢复积压任务放到监听就绪之后后台执行，避免启动被大量外网任务阻塞
    //（对齐上游：Web 进程启动不等待任务队列）。
    tokio::spawn(async move {
        let recovered = queue.recover_pending().await.unwrap_or(0);
        if recovered > 0 {
            println!("已恢复 {recovered} 个积压任务，后台处理中");
        }
    });
    match axum::serve(listener, app).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("服务异常退出: {e}");
            2
        }
    }
}

/// 按会话存储类型构建会话管理层（SQLite/PostgreSQL 共用过期策略）。
fn build_session_layer<S>(store: S, session_ttl_secs: u64) -> tower_sessions::SessionManagerLayer<S>
where
    S: tower_sessions::session_store::SessionStore,
{
    use tower_sessions::{Expiry, SessionManagerLayer};
    SessionManagerLayer::new(store)
        .with_secure(false)
        .with_expiry(Expiry::OnInactivity(time::Duration::seconds(
            session_ttl_secs as i64,
        )))
}

/// `init`：完整项目初始化（spec cli：init 完整初始化）。
/// 流程：生成配置文件（已存在则不覆盖）→ 创建数据目录 → 按 `database_url`
/// 连接数据库并执行迁移 →（可选）预置管理员。每步输出结果；幂等，重复执行安全。
fn cmd_init(args: &[String]) -> i32 {
    // 参数解析：[配置文件路径] / --admin-password <密码>。
    let mut config_path: Option<&str> = None;
    let mut admin_password: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--admin-password" => match args.get(i + 1) {
                Some(p) => {
                    admin_password = Some(p.clone());
                    i += 2;
                }
                None => {
                    eprintln!("--admin-password 需要一个口令参数");
                    return 2;
                }
            },
            "--help" | "-h" => {
                print_help();
                return 0;
            }
            other if other.starts_with('-') => {
                eprintln!("未知参数: {other}");
                return 2;
            }
            other if config_path.is_none() => {
                config_path = Some(other);
                i += 1;
            }
            other => {
                eprintln!("多余的位置参数: {other}");
                return 2;
            }
        }
    }

    // 1. 配置文件（已存在则不覆盖；自动创建父目录）。
    let path = Path::new(config_path.unwrap_or("config.toml"));
    if path.exists() {
        println!("配置文件已存在，跳过: {}", path.display());
    } else {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("创建配置文件目录失败 ({}): {e}", parent.display());
            return 2;
        }
        if let Err(e) = std::fs::write(path, config::sample_config()) {
            eprintln!("写入配置失败: {e}");
            return 2;
        }
        println!("已生成示例配置: {}", path.display());
    }

    // 2. 加载配置（环境变量覆盖生效；数据目录随之确定）。
    let cfg = Config::load();
    // 3. 数据目录。
    if let Err(e) = std::fs::create_dir_all(&cfg.data_dir) {
        eprintln!("创建数据目录失败 ({}): {e}", cfg.data_dir.display());
        return 2;
    }
    println!("数据目录就绪: {}", cfg.data_dir.display());

    // 4. 数据库连接 + 迁移（connect_any 内自动执行对应方言迁移）。
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("创建 tokio runtime 失败");
    runtime.block_on(async move {
        let sqlite_url = format!(
            "sqlite://{}?mode=rwc",
            cfg.data_dir.join("linklist.db").display()
        );
        let pool = match crate::db::connect_any(&cfg.database_url, &sqlite_url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("数据库连接失败: {e}");
                return 2;
            }
        };
        println!(
            "数据库就绪: {}（迁移已执行）",
            match pool {
                crate::db::DbPool::Sqlite(_) => format!("SQLite {}", sqlite_url),
                crate::db::DbPool::Postgres(_) => format!("PostgreSQL {}", cfg.database_url),
                crate::db::DbPool::MySql(_) => format!("MySQL {}", cfg.database_url),
            }
        );

        // 5. 预置管理员（--admin-password 优先于 LINKLIST_ADMIN_PASSWORD；仅无用户时生效）。
        let pw = admin_password.or_else(|| cfg.admin_password.clone());
        match pw {
            Some(p) if !p.is_empty() => {
                if p.len() < 8 {
                    eprintln!("管理员口令至少 8 位");
                    return 2;
                }
                match crate::models::provision_admin(&pool, "admin", &p).await {
                    Ok(Some(user)) => {
                        println!("已预置管理员账号: {} (id={})", user.username, user.id);
                    }
                    Ok(None) => println!("库中已有用户，跳过管理员预置"),
                    Err(e) => {
                        eprintln!("预置管理员失败: {e}");
                        return 2;
                    }
                }
            }
            _ => println!("未指定管理员口令，跳过预置（首次打开 Web 界面注册首个用户即管理员）"),
        }
        0
    })
}

/// `reset-password <username>`：交互式重置用户口令（rpassword 不回显，三种后端均支持）。
fn cmd_reset_password(username: Option<&str>) -> i32 {
    let Some(username) = username else {
        eprintln!("用法: linklist reset-password <username>");
        return 2;
    };
    let password = match rpassword::prompt_password("新口令: ") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!("口令不能为空");
            return 2;
        }
    };
    let confirm = match rpassword::prompt_password("确认口令: ") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("读取口令失败: {e}");
            return 2;
        }
    };
    if password != confirm {
        eprintln!("两次输入不一致");
        return 2;
    }
    if password.len() < 8 {
        eprintln!("口令至少 8 位");
        return 2;
    }

    let cfg = Config::load();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("创建 tokio runtime 失败");
    runtime.block_on(async move {
        let sqlite_url = format!(
            "sqlite://{}?mode=rwc",
            cfg.data_dir.join("linklist.db").display()
        );
        let pool = match crate::db::connect_any(&cfg.database_url, &sqlite_url).await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("数据库连接失败: {e}");
                return 2;
            }
        };
        let user_id = match crate::models::find_user_id_by_name(&pool, username).await {
            Ok(Some(id)) => id,
            Ok(None) => {
                eprintln!("用户不存在: {username}");
                return 2;
            }
            Err(e) => {
                eprintln!("查询用户失败: {e}");
                return 2;
            }
        };
        if let Err(e) = crate::models::update_password(&pool, user_id, &password).await {
            eprintln!("更新口令失败: {e}");
            return 2;
        }
        println!("已重置用户 {username} 的口令（id={user_id}）");
        0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_bad_option_returns_exit_code_2() {
        // 未知参数 → 2。
        assert_eq!(cmd_init(&["--nope".to_string()]), 2);
        // --admin-password 缺参数 → 2。
        assert_eq!(cmd_init(&["--admin-password".to_string()]), 2);
    }

    #[test]
    fn init_does_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# existing").unwrap();
        // 数据目录指向临时目录，避免触碰仓库 ./data。
        // SAFETY: 测试专用；该变量仅由本测试与被测代码读写，测试用例间无并发读写依赖。
        unsafe { std::env::set_var("LINKLIST_DATA_DIR", dir.path().join("data")) };
        // 已存在 → 不覆盖，返回 0（继续完成数据目录/数据库初始化）。
        assert_eq!(cmd_init(&[path.to_str().unwrap().to_string()]), 0);
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# existing");
        // SAFETY: 测试专用；仅清除本测试自行设置的变量，无并发读写依赖。
        // SAFETY: 测试专用；仅清除本测试自行设置的变量，无并发读写依赖。
        unsafe { std::env::remove_var("LINKLIST_DATA_DIR") };
    }

    #[test]
    fn init_creates_config_data_dir_and_sqlite_db() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        // 全新目录：父目录自动创建 + 数据目录创建 + SQLite 迁移 + 跳过管理员预置。
        // SAFETY: 测试专用；该变量仅由本测试与被测代码读写，测试用例间无并发读写依赖。
        unsafe { std::env::set_var("LINKLIST_DATA_DIR", dir.path().join("data")) };
        assert_eq!(cmd_init(&[path.to_str().unwrap().to_string()]), 0);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("listen"));
        assert!(
            dir.path().join("data/linklist.db").exists(),
            "SQLite 数据库应已创建"
        );
        // SAFETY: 测试专用；仅清除本测试自行设置的变量，无并发读写依赖。
        unsafe { std::env::remove_var("LINKLIST_DATA_DIR") };
    }

    #[test]
    fn init_admin_password_provisions_admin_when_empty() {
        // 空库 + --admin-password → 创建 admin 账号；重复执行幂等跳过。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // 用 LINKLIST_DATA_DIR 指向临时目录（apply_env 读取环境变量）。
        // SAFETY: 测试专用；该变量仅由本测试与被测代码读写，测试用例间无并发读写依赖。
        unsafe { std::env::set_var("LINKLIST_DATA_DIR", dir.path().join("data")) };
        assert_eq!(
            cmd_init(&[
                path.to_str().unwrap().to_string(),
                "--admin-password".to_string(),
                "password123".to_string(),
            ]),
            0
        );
        let db_path = dir.path().join("data/linklist.db");
        assert!(
            db_path.exists(),
            "SQLite 数据库应已创建: {}",
            db_path.display()
        );
        // 幂等：再次执行（含预置）应成功且不报错。
        assert_eq!(
            cmd_init(&[
                path.to_str().unwrap().to_string(),
                "--admin-password".to_string(),
                "password123".to_string(),
            ]),
            0
        );
        // SAFETY: 测试专用；仅清除本测试自行设置的变量，无并发读写依赖。
        unsafe { std::env::remove_var("LINKLIST_DATA_DIR") };
    }

    #[tokio::test]
    async fn provision_admin_skips_when_users_exist() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        // 空库 → 创建成功；已有用户 → None 跳过。
        let created = crate::models::provision_admin(&pool, "admin", "password123")
            .await
            .unwrap();
        assert!(created.is_some(), "空库应创建管理员");
        assert!(created.unwrap().is_admin);
        let skipped = crate::models::provision_admin(&pool, "admin", "password123")
            .await
            .unwrap();
        assert!(skipped.is_none(), "已有用户时应跳过预置");
    }

    #[tokio::test]
    async fn reset_password_updates_hash_and_unknown_user_errors() {
        let pool = crate::db::connect_in_memory().await.unwrap();
        let _ = crate::models::create_user(&pool, "alice", "oldpass123", false)
            .await
            .unwrap();
        let uid = crate::models::find_user_id_by_name(&pool, "alice")
            .await
            .unwrap()
            .expect("应找到用户");
        crate::models::update_password(&pool, uid, "newpass456")
            .await
            .unwrap();
        // 新口令哈希可验证登录（与登录一致的 Argon2id 路径）。
        let row = crate::db::q("SELECT password_hash FROM users WHERE id = ?")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(crate::auth::password::verify_password("newpass456", &row.get_string(0)).is_ok());
        // 不存在用户 → None。
        assert!(
            crate::models::find_user_id_by_name(&pool, "nobody")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn session_layer_builds_for_sqlite_store() {
        // 会话层三种存储共用 build_session_layer（泛型单路径，过期策略一致由实现保证）。
        // SQLite 内存库构造 SqliteStore 走一遍构建；Postgres/MySql 分支为同构泛型调用。
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let pool = sqlx::pool::PoolOptions::<sqlx::Sqlite>::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            let store = tower_sessions_sqlx_store::SqliteStore::new(pool);
            let _layer = build_session_layer(store, 604800);
        });
    }
}
