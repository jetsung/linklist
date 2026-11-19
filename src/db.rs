// 数据库辅助：连接/迁移/初始化检查（部分 API 供测试与嵌入场景使用）。
// 支持三后端：SQLite（默认）、PostgreSQL（`postgres://` 前缀）与 MySQL（`mysql://` 前缀）。
#![allow(dead_code)]

use sqlx::Row;

use crate::error::AppError;

/// 数据库连接池（SQLite / PostgreSQL / MySQL 三选一）。
#[derive(Clone, Debug)]
pub enum DbPool {
    Sqlite(sqlx::SqlitePool),
    Postgres(sqlx::PgPool),
    MySql(sqlx::MySqlPool),
}

/// 数据库行（SQLite / PostgreSQL / MySQL 三选一）。
pub enum DbRow {
    Sqlite(sqlx::sqlite::SqliteRow),
    Postgres(sqlx::postgres::PgRow),
    MySql(sqlx::mysql::MySqlRow),
}

/// 单条 SQL 执行结果。
pub enum DbQueryResult {
    Sqlite(sqlx::sqlite::SqliteQueryResult),
    Postgres(sqlx::postgres::PgQueryResult),
    MySql(sqlx::mysql::MySqlQueryResult),
}

impl DbQueryResult {
    pub fn rows_affected(&self) -> u64 {
        match self {
            DbQueryResult::Sqlite(r) => r.rows_affected(),
            DbQueryResult::Postgres(r) => r.rows_affected(),
            DbQueryResult::MySql(r) => r.rows_affected(),
        }
    }

    /// 取自增主键（仅 SQLite/MySQL 支持；PG 请改写 SQL 为 `RETURNING id` 后用 fetch_one 读取）。
    pub fn last_insert_rowid(&self) -> i64 {
        match self {
            DbQueryResult::Sqlite(r) => r.last_insert_rowid(),
            DbQueryResult::MySql(r) => r.last_insert_id() as i64,
            DbQueryResult::Postgres(_) => {
                panic!("PostgreSQL 不支持 last_insert_rowid；请使用 RETURNING id")
            }
        }
    }
}

/// 跨后端事务（包装 SQLite / PostgreSQL / MySQL 事务连接）。
pub enum DbTx {
    Sqlite(sqlx::Transaction<'static, sqlx::Sqlite>),
    Postgres(sqlx::Transaction<'static, sqlx::Postgres>),
    MySql(sqlx::Transaction<'static, sqlx::MySql>),
}

impl DbPool {
    /// 开启事务。
    pub async fn begin(&self) -> Result<DbTx, AppError> {
        match self {
            DbPool::Sqlite(p) => Ok(DbTx::Sqlite(p.begin().await.map_err(AppError::from)?)),
            DbPool::Postgres(p) => Ok(DbTx::Postgres(p.begin().await.map_err(AppError::from)?)),
            DbPool::MySql(p) => Ok(DbTx::MySql(p.begin().await.map_err(AppError::from)?)),
        }
    }
}

impl DbTx {
    /// 提交事务。
    pub async fn commit(self) -> Result<(), AppError> {
        match self {
            DbTx::Sqlite(tx) => tx.commit().await.map_err(AppError::from),
            DbTx::Postgres(tx) => tx.commit().await.map_err(AppError::from),
            DbTx::MySql(tx) => tx.commit().await.map_err(AppError::from),
        }
    }

    /// 回滚事务（放弃时自动回滚，无需显式调用）。
    #[allow(dead_code)]
    pub async fn rollback(self) -> Result<(), AppError> {
        match self {
            DbTx::Sqlite(tx) => tx.rollback().await.map_err(AppError::from),
            DbTx::Postgres(tx) => tx.rollback().await.map_err(AppError::from),
            DbTx::MySql(tx) => tx.rollback().await.map_err(AppError::from),
        }
    }

    fn sqlite_conn(&mut self) -> &mut sqlx::SqliteConnection {
        match self {
            DbTx::Sqlite(tx) => tx,
            _ => panic!("事务后端不匹配"),
        }
    }

    fn pg_conn(&mut self) -> &mut sqlx::postgres::PgConnection {
        match self {
            DbTx::Postgres(tx) => tx,
            _ => panic!("事务后端不匹配"),
        }
    }

    fn mysql_conn(&mut self) -> &mut sqlx::mysql::MySqlConnection {
        match self {
            DbTx::MySql(tx) => tx,
            _ => panic!("事务后端不匹配"),
        }
    }
}

/// 可在 SQLite 与 PostgreSQL 上执行的查询建造器。
///
/// 对 `sqlx::query(sql).bind(v)` 的轻量封装：先攒 SQL 与绑定值，
/// `fetch_*`/`execute` 时按池类型分派。
#[derive(Default)]
pub struct DbQuery {
    sql: String,
    binds: Vec<DbValue>,
}

/// 支持的绑定值类型（覆盖本项目的取值域）。
#[derive(Clone, Debug)]
pub enum DbValue {
    Null,
    I64(i64),
    U32(u32),
    F64(f64),
    Bool(bool),
    Text(String),
    Bytes(Vec<u8>),
}

impl DbQuery {
    pub fn new(sql: impl Into<String>) -> Self {
        Self {
            sql: sql.into(),
            binds: Vec::new(),
        }
    }

    /// 追加绑定值（借用形式，值被克隆收编）。
    pub fn bind<T: Into<DbValue>>(mut self, v: T) -> Self {
        self.binds.push(v.into());
        self
    }

    pub fn sql(&self) -> &str {
        &self.sql
    }

    pub fn binds(&self) -> &[DbValue] {
        &self.binds
    }

    fn sqlite_query(&self) -> sqlx::query::Query<'_, sqlx::Sqlite, sqlx::sqlite::SqliteArguments> {
        // SQL 由代码内常量拼接 + 绑定参数化，无外部输入直接入串，视为已审计。
        let mut q = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()));
        for b in &self.binds {
            q = match b {
                DbValue::Null => q.bind(None::<i64>),
                DbValue::I64(v) => q.bind(*v),
                DbValue::U32(v) => q.bind(*v),
                DbValue::F64(v) => q.bind(*v),
                DbValue::Bool(v) => q.bind(*v),
                DbValue::Text(v) => q.bind(v.as_str()),
                DbValue::Bytes(v) => q.bind(v.as_slice()),
            };
        }
        q
    }

    /// 重写占位符后在 PG 上执行（内部完成构造与执行，规避跨层生命周期）。
    async fn pg_exec<'c, E>(&self, executor: E, action: PgAction) -> Result<PgOutcome, AppError>
    where
        E: sqlx::Executor<'c, Database = sqlx::Postgres>,
    {
        let sql = pg_rewrite_placeholders(&self.sql);
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql));
        for b in &self.binds {
            q = match b {
                DbValue::Null => q.bind(None::<i64>),
                DbValue::I64(v) => q.bind(*v),
                // PG 驱动无 u32 encode，统一按 i64 绑定。
                DbValue::U32(v) => q.bind(i64::from(*v)),
                DbValue::F64(v) => q.bind(*v),
                DbValue::Bool(v) => q.bind(*v),
                DbValue::Text(v) => q.bind(v.as_str()),
                DbValue::Bytes(v) => q.bind(v.as_slice()),
            };
        }
        match action {
            PgAction::FetchOne => q
                .fetch_one(executor)
                .await
                .map(DbRow::Postgres)
                .map(PgOutcome::Row),
            PgAction::FetchOptional => q
                .fetch_optional(executor)
                .await
                .map(|r| r.map(DbRow::Postgres))
                .map(PgOutcome::OptRow),
            PgAction::FetchAll => q
                .fetch_all(executor)
                .await
                .map(|rs| rs.into_iter().map(DbRow::Postgres).collect())
                .map(PgOutcome::Rows),
            PgAction::Execute => q
                .execute(executor)
                .await
                .map(DbQueryResult::Postgres)
                .map(PgOutcome::Result),
        }
        .map_err(AppError::from)
    }

    fn mysql_query(&self) -> sqlx::query::Query<'_, sqlx::MySql, sqlx::mysql::MySqlArguments> {
        let mut q = sqlx::query(sqlx::AssertSqlSafe(self.sql.clone()));
        for b in &self.binds {
            q = match b {
                DbValue::Null => q.bind(None::<i64>),
                DbValue::I64(v) => q.bind(*v),
                DbValue::U32(v) => q.bind(*v),
                DbValue::F64(v) => q.bind(*v),
                DbValue::Bool(v) => q.bind(*v),
                DbValue::Text(v) => q.bind(v.as_str()),
                DbValue::Bytes(v) => q.bind(v.as_slice()),
            };
        }
        q
    }

    pub async fn fetch_one(&self, pool: &DbPool) -> Result<DbRow, AppError> {
        match pool {
            DbPool::Sqlite(p) => self
                .sqlite_query()
                .fetch_one(p)
                .await
                .map(DbRow::Sqlite)
                .map_err(AppError::from),
            DbPool::Postgres(p) => match self.pg_exec(p, PgAction::FetchOne).await? {
                PgOutcome::Row(r) => Ok(r),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbPool::MySql(p) => self
                .mysql_query()
                .fetch_one(p)
                .await
                .map(DbRow::MySql)
                .map_err(AppError::from),
        }
    }

    pub async fn fetch_optional(&self, pool: &DbPool) -> Result<Option<DbRow>, AppError> {
        match pool {
            DbPool::Sqlite(p) => self
                .sqlite_query()
                .fetch_optional(p)
                .await
                .map(|r| r.map(DbRow::Sqlite))
                .map_err(AppError::from),
            DbPool::Postgres(p) => match self.pg_exec(p, PgAction::FetchOptional).await? {
                PgOutcome::OptRow(r) => Ok(r),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbPool::MySql(p) => self
                .mysql_query()
                .fetch_optional(p)
                .await
                .map(|r| r.map(DbRow::MySql))
                .map_err(AppError::from),
        }
    }

    pub async fn fetch_all(&self, pool: &DbPool) -> Result<Vec<DbRow>, AppError> {
        match pool {
            DbPool::Sqlite(p) => self
                .sqlite_query()
                .fetch_all(p)
                .await
                .map(|rs| rs.into_iter().map(DbRow::Sqlite).collect())
                .map_err(AppError::from),
            DbPool::Postgres(p) => match self.pg_exec(p, PgAction::FetchAll).await? {
                PgOutcome::Rows(rs) => Ok(rs),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbPool::MySql(p) => self
                .mysql_query()
                .fetch_all(p)
                .await
                .map(|rs| rs.into_iter().map(DbRow::MySql).collect())
                .map_err(AppError::from),
        }
    }

    pub async fn execute(&self, pool: &DbPool) -> Result<DbQueryResult, AppError> {
        match pool {
            DbPool::Sqlite(p) => self
                .sqlite_query()
                .execute(p)
                .await
                .map(DbQueryResult::Sqlite)
                .map_err(AppError::from),
            DbPool::Postgres(p) => match self.pg_exec(p, PgAction::Execute).await? {
                PgOutcome::Result(r) => Ok(r),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbPool::MySql(p) => self
                .mysql_query()
                .execute(p)
                .await
                .map(DbQueryResult::MySql)
                .map_err(AppError::from),
        }
    }

    /// 事务内执行（返回单行）。
    pub async fn fetch_one_tx(&self, tx: &mut DbTx) -> Result<DbRow, AppError> {
        match tx {
            DbTx::Sqlite(_) => self
                .sqlite_query()
                .fetch_one(tx.sqlite_conn())
                .await
                .map(DbRow::Sqlite)
                .map_err(AppError::from),
            DbTx::Postgres(_) => match self.pg_exec(tx.pg_conn(), PgAction::FetchOne).await? {
                PgOutcome::Row(r) => Ok(r),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbTx::MySql(_) => self
                .mysql_query()
                .fetch_one(tx.mysql_conn())
                .await
                .map(DbRow::MySql)
                .map_err(AppError::from),
        }
    }

    /// 事务内执行（返回可选单行）。
    pub async fn fetch_optional_tx(&self, tx: &mut DbTx) -> Result<Option<DbRow>, AppError> {
        match tx {
            DbTx::Sqlite(_) => self
                .sqlite_query()
                .fetch_optional(tx.sqlite_conn())
                .await
                .map(|r| r.map(DbRow::Sqlite))
                .map_err(AppError::from),
            DbTx::Postgres(_) => match self.pg_exec(tx.pg_conn(), PgAction::FetchOptional).await? {
                PgOutcome::OptRow(r) => Ok(r),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbTx::MySql(_) => self
                .mysql_query()
                .fetch_optional(tx.mysql_conn())
                .await
                .map(|r| r.map(DbRow::MySql))
                .map_err(AppError::from),
        }
    }

    /// 事务内执行（返回多行）。
    pub async fn fetch_all_tx(&self, tx: &mut DbTx) -> Result<Vec<DbRow>, AppError> {
        match tx {
            DbTx::Sqlite(_) => self
                .sqlite_query()
                .fetch_all(tx.sqlite_conn())
                .await
                .map(|rs| rs.into_iter().map(DbRow::Sqlite).collect())
                .map_err(AppError::from),
            DbTx::Postgres(_) => match self.pg_exec(tx.pg_conn(), PgAction::FetchAll).await? {
                PgOutcome::Rows(rs) => Ok(rs),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbTx::MySql(_) => self
                .mysql_query()
                .fetch_all(tx.mysql_conn())
                .await
                .map(|rs| rs.into_iter().map(DbRow::MySql).collect())
                .map_err(AppError::from),
        }
    }

    /// 事务内执行写操作。
    pub async fn execute_tx(&self, tx: &mut DbTx) -> Result<DbQueryResult, AppError> {
        match tx {
            DbTx::Sqlite(_) => self
                .sqlite_query()
                .execute(tx.sqlite_conn())
                .await
                .map(DbQueryResult::Sqlite)
                .map_err(AppError::from),
            DbTx::Postgres(_) => match self.pg_exec(tx.pg_conn(), PgAction::Execute).await? {
                PgOutcome::Result(r) => Ok(r),
                _ => unreachable!("PG 分派动作与结果不匹配"),
            },
            DbTx::MySql(_) => self
                .mysql_query()
                .execute(tx.mysql_conn())
                .await
                .map(DbQueryResult::MySql)
                .map_err(AppError::from),
        }
    }
}
/// 数据库方言（SQL 构造时按方言选择等价写法）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbDialect {
    Sqlite,
    Postgres,
    MySql,
}

impl DbTx {
    /// 当前事务对应的方言。
    pub fn dialect(&self) -> DbDialect {
        match self {
            DbTx::Sqlite(_) => DbDialect::Sqlite,
            DbTx::Postgres(_) => DbDialect::Postgres,
            DbTx::MySql(_) => DbDialect::MySql,
        }
    }
}

impl DbPool {
    /// 当前连接池对应的方言。
    pub fn dialect(&self) -> DbDialect {
        match self {
            DbPool::Sqlite(_) => DbDialect::Sqlite,
            DbPool::Postgres(_) => DbDialect::Postgres,
            DbPool::MySql(_) => DbDialect::MySql,
        }
    }
}

/// 多值聚合拼接为分隔字符串（tags_agg 等场景）的三方言等价写法：
/// SQLite `GROUP_CONCAT(expr, sep)`；MySQL `GROUP_CONCAT(expr SEPARATOR sep)`；
/// PostgreSQL `string_agg(expr, sep)`。
pub fn sql_concat_agg(dialect: DbDialect, expr: &str, sep: &str) -> String {
    match dialect {
        DbDialect::Sqlite => format!("GROUP_CONCAT({expr}, '{sep}')"),
        DbDialect::MySql => format!("GROUP_CONCAT({expr} SEPARATOR '{sep}')"),
        DbDialect::Postgres => format!("string_agg({expr}, '{sep}')"),
    }
}

/// "插入或忽略"幂等写入的三方言等价 SQL：SQLite `INSERT OR IGNORE`；
/// MySQL `INSERT IGNORE`；PostgreSQL `ON CONFLICT DO NOTHING`。
/// `into` 形如 `bookmark_tags (bookmark_id, tag_id)`，`values` 形如 `?, ?`。
pub fn sql_insert_ignore(dialect: DbDialect, into: &str, values: &str) -> String {
    match dialect {
        DbDialect::MySql => format!("INSERT IGNORE INTO {into} VALUES ({values})"),
        DbDialect::Postgres => {
            format!("INSERT INTO {into} VALUES ({values}) ON CONFLICT DO NOTHING")
        }
        DbDialect::Sqlite => format!("INSERT OR IGNORE INTO {into} VALUES ({values})"),
    }
}

/// settings 键值上翻的三方言等价 SQL（`key` 在 MySQL 中为保留字，需反引号；
/// MySQL 用 ON DUPLICATE KEY UPDATE + VALUES(col)，SQLite/PG 用 excluded）。
pub fn sql_settings_upsert(dialect: DbDialect) -> &'static str {
    match dialect {
        DbDialect::MySql => {
            "INSERT INTO settings (`key`, value) VALUES (?, ?) ON DUPLICATE KEY UPDATE value = VALUES(value)"
        }
        _ => {
            "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value"
        }
    }
}

/// favicon 主机上翻的三方言等价 SQL（与 sql_settings_upsert 同构）。
pub fn sql_favicon_upsert(dialect: DbDialect) -> &'static str {
    match dialect {
        DbDialect::MySql => {
            "INSERT INTO favicons (host, data, updated_at) VALUES (?, ?, ?) \
             ON DUPLICATE KEY UPDATE data = VALUES(data), updated_at = VALUES(updated_at)"
        }
        _ => {
            "INSERT INTO favicons (host, data, updated_at) VALUES (?, ?, ?) \
              ON CONFLICT(host) DO UPDATE SET data = excluded.data, updated_at = excluded.updated_at"
        }
    }
}

/// PG 分派动作。
#[derive(Clone, Copy)]
enum PgAction {
    FetchOne,
    FetchOptional,
    FetchAll,
    Execute,
}

/// PG 分派执行结果。
enum PgOutcome {
    Row(DbRow),
    OptRow(Option<DbRow>),
    Rows(Vec<DbRow>),
    Result(DbQueryResult),
}

/// 把通用 `?` 占位符重写为 PostgreSQL 的 `$N`。
///
/// sqlx 的 PG 驱动不重写占位符（与 SQLite/MySQL 不同），跨后端 SQL 统一用 `?`
/// 风格书写，在 PG 分派处转换；字符串字面量（含 `''` 转义）与 `"…"` 标识符内的
/// `?` 原样保留。
#[allow(clippy::while_let_on_iterator)] // 循环内 peek() 需要借用 chars，改 for 会冲突
fn pg_rewrite_placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut n = 0usize;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                // 字符串字面量：'' 转义跳过。
                out.push(c);
                while let Some(c2) = chars.next() {
                    out.push(c2);
                    if c2 == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            out.push('\'');
                        } else {
                            break;
                        }
                    }
                }
            }
            '"' => {
                out.push(c);
                while let Some(c2) = chars.next() {
                    out.push(c2);
                    if c2 == '"' {
                        break;
                    }
                }
            }
            '?' => {
                n += 1;
                out.push_str(&format!("${n}"));
            }
            _ => out.push(c),
        }
    }
    out
}

/// 构造跨后端查询（对齐原 `sqlx::query(sql)` 调用点）。
pub fn q(sql: impl Into<String>) -> DbQuery {
    DbQuery::new(sql)
}

/// 构造跨后端标量查询（对齐原 `sqlx::query_scalar(sql)` 调用点）。
/// 返回类型经 `DbRow` 读取，泛型参数仅保留原调用形。
#[allow(clippy::extra_unused_type_parameters)]
pub fn q_scalar<T>(sql: impl Into<String>) -> DbQuery {
    DbQuery::new(sql)
}

/// 构造跨后端查询（对齐原 `sqlx::query_as::<_, T>(sql)` 调用点）。
/// 行映射由调用方以 `DbRow` 手工完成，本函数只负责攒 SQL/绑定。
#[allow(clippy::extra_unused_type_parameters)]
pub fn q_as<T>(sql: impl Into<String>) -> DbQuery {
    DbQuery::new(sql)
}

/// 标量查询便捷函数。
pub async fn query_scalar_i64(sql: &str, pool: &DbPool) -> Result<i64, AppError> {
    let row = DbQuery::new(sql).fetch_one(pool).await?;
    Ok(row.get_i64(0))
}

impl DbRow {
    pub fn get_i64(&self, idx: usize) -> i64 {
        match self {
            DbRow::Sqlite(r) => r.get::<i64, _>(idx),
            // PG 里 BOOLEAN/SERIAL 等列经 PG 驱动读取；数值统一转 i64。
            DbRow::Postgres(r) => {
                // 尝试 i64，失败再试 BOOLEAN（0/1）与 i32（SERIAL/INTEGER）。
                if let Ok(v) = r.try_get::<i64, _>(idx) {
                    v
                } else if let Ok(v) = r.try_get::<bool, _>(idx) {
                    i64::from(v)
                } else {
                    i64::from(r.get::<i32, _>(idx))
                }
            }
            // MySQL BIGINT 直读；INT/MEDIUMINT/SMALLINT/TINYINT 需逐级 fallback。
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<i64, _>(idx) {
                    v
                } else if let Ok(v) = r.try_get::<i32, _>(idx) {
                    i64::from(v)
                } else {
                    i64::from(r.get::<i16, _>(idx))
                }
            }
        }
    }

    pub fn get_u32(&self, idx: usize) -> u32 {
        match self {
            DbRow::Sqlite(r) => r.get::<i64, _>(idx) as u32,
            DbRow::Postgres(r) => r.get::<i32, _>(idx) as u32,
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<i64, _>(idx) {
                    v as u32
                } else {
                    i64::from(r.try_get::<i32, _>(idx).unwrap_or_default()) as u32
                }
            }
        }
    }

    pub fn get_string(&self, idx: usize) -> String {
        match self {
            DbRow::Sqlite(r) => r.get::<String, _>(idx),
            DbRow::Postgres(r) => pg_string(r, idx),
            DbRow::MySql(r) => r.get::<String, _>(idx),
        }
    }

    pub fn get_opt_string(&self, idx: usize) -> Option<String> {
        match self {
            DbRow::Sqlite(r) => r.get::<Option<String>, _>(idx),
            DbRow::Postgres(r) => pg_opt_string(r, idx),
            DbRow::MySql(r) => r.get::<Option<String>, _>(idx),
        }
    }

    pub fn get_opt_i64(&self, idx: usize) -> Option<i64> {
        match self {
            DbRow::Sqlite(r) => r.get::<Option<i64>, _>(idx),
            DbRow::Postgres(r) => {
                if let Ok(v) = r.try_get::<Option<i64>, _>(idx) {
                    v
                } else {
                    r.get::<Option<i32>, _>(idx).map(i64::from)
                }
            }
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<Option<i64>, _>(idx) {
                    v
                } else if let Ok(v) = r.try_get::<Option<i32>, _>(idx) {
                    v.map(i64::from)
                } else {
                    r.try_get::<Option<i16>, _>(idx)
                        .unwrap_or_default()
                        .map(i64::from)
                }
            }
        }
    }

    /// 按列名取值（SQLite/PG/MySQL 均支持列名定位）。
    pub fn get_named_i64(&self, name: &str) -> i64 {
        match self {
            DbRow::Sqlite(r) => r.get::<i64, _>(name),
            DbRow::Postgres(r) => {
                if let Ok(v) = r.try_get::<i64, _>(name) {
                    v
                } else if let Ok(v) = r.try_get::<bool, _>(name) {
                    // PG BOOLEAN 列（is_admin 等）按 0/1 读取。
                    i64::from(v)
                } else {
                    i64::from(r.get::<i32, _>(name))
                }
            }
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<i64, _>(name) {
                    v
                } else if let Ok(v) = r.try_get::<i32, _>(name) {
                    i64::from(v)
                } else {
                    i64::from(r.get::<i16, _>(name))
                }
            }
        }
    }

    pub fn get_named_string(&self, name: &str) -> String {
        match self {
            DbRow::Sqlite(r) => r.get::<String, _>(name),
            DbRow::Postgres(r) => pg_string_named(r, name),
            DbRow::MySql(r) => r.get::<String, _>(name),
        }
    }

    pub fn get_named_opt_string(&self, name: &str) -> Option<String> {
        match self {
            DbRow::Sqlite(r) => r.get::<Option<String>, _>(name),
            DbRow::Postgres(r) => pg_opt_string_named(r, name),
            DbRow::MySql(r) => r.get::<Option<String>, _>(name),
        }
    }

    pub fn get_u32_named(&self, name: &str) -> u32 {
        match self {
            DbRow::Sqlite(r) => r.get::<i64, _>(name) as u32,
            DbRow::Postgres(r) => r.get::<i32, _>(name) as u32,
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<i64, _>(name) {
                    v as u32
                } else {
                    i64::from(r.try_get::<i32, _>(name).unwrap_or_default()) as u32
                }
            }
        }
    }

    pub fn get_named_opt_i64(&self, name: &str) -> Option<i64> {
        match self {
            DbRow::Sqlite(r) => r.get::<Option<i64>, _>(name),
            DbRow::Postgres(r) => {
                if let Ok(v) = r.try_get::<Option<i64>, _>(name) {
                    v
                } else if let Ok(v) = r.try_get::<Option<bool>, _>(name) {
                    v.map(i64::from)
                } else {
                    r.get::<Option<i32>, _>(name).map(i64::from)
                }
            }
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<Option<i64>, _>(name) {
                    v
                } else if let Ok(v) = r.try_get::<Option<i32>, _>(name) {
                    v.map(i64::from)
                } else {
                    r.try_get::<Option<i16>, _>(name)
                        .unwrap_or_default()
                        .map(i64::from)
                }
            }
        }
    }

    /// 取字符串列，列缺失或类型不符时返回默认值（对齐原 try_get + unwrap_or_else 语义）。
    pub fn try_get_string_or(&self, name: &str, default: &str) -> String {
        match self {
            DbRow::Sqlite(r) => r
                .try_get::<String, _>(name)
                .unwrap_or_else(|_| default.to_string()),
            DbRow::Postgres(r) => r
                .try_get::<String, _>(name)
                .unwrap_or_else(|_| default.to_string()),
            DbRow::MySql(r) => r
                .try_get::<String, _>(name)
                .unwrap_or_else(|_| default.to_string()),
        }
    }

    /// 取整型布尔列（SQLite INTEGER 0/1；PG BOOLEAN；MySQL TINYINT 0/1）。
    pub fn get_named_bool(&self, name: &str) -> bool {
        match self {
            DbRow::Sqlite(r) => r.get::<i64, _>(name) != 0,
            DbRow::Postgres(r) => {
                if let Ok(v) = r.try_get::<bool, _>(name) {
                    v
                } else {
                    r.get::<i64, _>(name) != 0
                }
            }
            DbRow::MySql(r) => {
                if let Ok(v) = r.try_get::<i64, _>(name) {
                    v != 0
                } else if let Ok(v) = r.try_get::<i32, _>(name) {
                    v != 0
                } else {
                    r.try_get::<bool, _>(name).unwrap_or(false)
                }
            }
        }
    }

    /// 读取 BLOB 列（SQLite BLOB / PG BYTEA / MySQL BLOB），空值返回空 Vec。
    pub fn get_named_bytes(&self, name: &str) -> Vec<u8> {
        match self {
            DbRow::Sqlite(r) => r.try_get::<Vec<u8>, _>(name).unwrap_or_default(),
            DbRow::Postgres(r) => r.try_get::<Vec<u8>, _>(name).unwrap_or_default(),
            DbRow::MySql(r) => r.try_get::<Vec<u8>, _>(name).unwrap_or_default(),
        }
    }
}

/// PG 字符串读取：TEXT/VARCHAR 直读；TIMESTAMPTZ/TIMESTAMP 等时间类型列
/// 解码为字符串（转 RFC3339，兼容迁移文件中时间列的时间类型）。
fn pg_string(r: &sqlx::postgres::PgRow, idx: usize) -> String {
    if let Ok(s) = r.try_get::<String, _>(idx) {
        return s;
    }
    pg_time_to_string(r, idx).unwrap_or_default()
}

fn pg_opt_string(r: &sqlx::postgres::PgRow, idx: usize) -> Option<String> {
    if let Ok(s) = r.try_get::<Option<String>, _>(idx) {
        return s;
    }
    pg_time_to_string(r, idx)
}

fn pg_string_named(r: &sqlx::postgres::PgRow, name: &str) -> String {
    if let Ok(s) = r.try_get::<String, _>(name) {
        return s;
    }
    pg_time_to_string(r, name).unwrap_or_default()
}

fn pg_opt_string_named(r: &sqlx::postgres::PgRow, name: &str) -> Option<String> {
    if let Ok(s) = r.try_get::<Option<String>, _>(name) {
        return s;
    }
    pg_time_to_string(r, name)
}

/// 把 PG 时间列（TIMESTAMPTZ/TIMESTAMP）解码为字符串（RFC3339，与 SQLite TEXT 时间格式对齐）。
fn pg_time_to_string<I>(r: &sqlx::postgres::PgRow, key: I) -> Option<String>
where
    I: sqlx::ColumnIndex<sqlx::postgres::PgRow> + Copy,
{
    if let Ok(v) = r.try_get::<chrono::DateTime<chrono::Utc>, _>(key) {
        return Some(v.to_rfc3339());
    }
    if let Ok(v) = r.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(key) {
        return v.map(|d| d.to_rfc3339());
    }
    if let Ok(v) = r.try_get::<chrono::NaiveDateTime, _>(key) {
        return Some(v.and_utc().to_rfc3339());
    }
    r.try_get::<Option<chrono::NaiveDateTime>, _>(key)
        .unwrap_or_default()
        .map(|d| d.and_utc().to_rfc3339())
}

impl From<i64> for DbValue {
    fn from(v: i64) -> Self {
        DbValue::I64(v)
    }
}
impl From<i32> for DbValue {
    fn from(v: i32) -> Self {
        DbValue::I64(i64::from(v))
    }
}
impl From<u32> for DbValue {
    fn from(v: u32) -> Self {
        DbValue::U32(v)
    }
}
impl From<usize> for DbValue {
    fn from(v: usize) -> Self {
        DbValue::I64(v as i64)
    }
}
impl From<bool> for DbValue {
    fn from(v: bool) -> Self {
        DbValue::Bool(v)
    }
}
impl From<f64> for DbValue {
    fn from(v: f64) -> Self {
        DbValue::F64(v)
    }
}
impl From<String> for DbValue {
    fn from(v: String) -> Self {
        DbValue::Text(v)
    }
}
impl From<&str> for DbValue {
    fn from(v: &str) -> Self {
        DbValue::Text(v.to_string())
    }
}
impl From<Option<String>> for DbValue {
    fn from(v: Option<String>) -> Self {
        match v {
            Some(s) => DbValue::Text(s),
            None => DbValue::Null,
        }
    }
}
impl From<Option<i64>> for DbValue {
    fn from(v: Option<i64>) -> Self {
        match v {
            Some(i) => DbValue::I64(i),
            None => DbValue::Null,
        }
    }
}
impl From<&String> for DbValue {
    fn from(v: &String) -> Self {
        DbValue::Text(v.clone())
    }
}
impl From<Option<&str>> for DbValue {
    fn from(v: Option<&str>) -> Self {
        match v {
            Some(s) => DbValue::Text(s.to_string()),
            None => DbValue::Null,
        }
    }
}
impl From<Option<u32>> for DbValue {
    fn from(v: Option<u32>) -> Self {
        match v {
            Some(i) => DbValue::I64(i as i64),
            None => DbValue::Null,
        }
    }
}
impl From<Option<bool>> for DbValue {
    fn from(v: Option<bool>) -> Self {
        match v {
            Some(b) => DbValue::Bool(b),
            None => DbValue::Null,
        }
    }
}
impl From<Vec<u8>> for DbValue {
    fn from(v: Vec<u8>) -> Self {
        DbValue::Bytes(v)
    }
}
impl From<&[u8]> for DbValue {
    fn from(v: &[u8]) -> Self {
        DbValue::Bytes(v.to_vec())
    }
}

/// 判断连接串是否指向 PostgreSQL。
pub fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

/// 判断连接串是否指向 MySQL。
pub fn is_mysql(url: &str) -> bool {
    url.starts_with("mysql://") || url.starts_with("mariadb://")
}

/// 按配置建立连接池并执行迁移。
///
/// 后端选取仅由 `database_url` 前缀决定：`postgres://|postgresql://` → PostgreSQL，
/// `mysql://|mariadb://` → MySQL，其余（含空串）→ SQLite（`sqlite_url`）。
pub async fn connect_any(database_url: &str, sqlite_url: &str) -> Result<DbPool, AppError> {
    if is_postgres(database_url) {
        let pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(|e| AppError::internal(format!("PostgreSQL 连接失败: {e}")))?;
        migrate_postgres(&pool).await?;
        return Ok(DbPool::Postgres(pool));
    }
    if is_mysql(database_url) {
        let pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(|e| AppError::internal(format!("MySQL 连接失败: {e}")))?;
        migrate_mysql(&pool).await?;
        return Ok(DbPool::MySql(pool));
    }
    // 用户显式配置的 sqlite:// 连接串优先于按 data_dir 推导的默认路径。
    if database_url.starts_with("sqlite://") || database_url.starts_with("sqlite:") {
        return connect(database_url).await;
    }
    // 未知协议（如 mssql://）明确报错，不退回 SQLite。
    // 用 BadRequest 携带完整详情：该错误发生在启动/init（面向配置操作者），
    // 而非匿名 HTTP 请求路径，内部错误变体会丢弃详情。
    if !database_url.is_empty() {
        return Err(AppError::BadRequest(format!(
            "不支持的数据库连接串协议: {database_url}（支持 sqlite://、postgres://、mysql://）"
        )));
    }
    connect(sqlite_url).await
}

/// 执行嵌入的数据库迁移（migrations/*.sql，SQLite 方言）。
pub async fn migrate(pool: &sqlx::SqlitePool) -> Result<(), AppError> {
    sqlx::migrate!("./migrations/sqlite")
        .run(pool)
        .await
        .map_err(|e| AppError::internal(format!("数据库迁移失败: {e}")))?;
    Ok(())
}

/// 执行 PostgreSQL 初始迁移（migrations/postgres/0001_init.sql，可重复执行）。
pub async fn migrate_postgres(pool: &sqlx::PgPool) -> Result<(), AppError> {
    let sql = include_str!("../migrations/postgres/0001_init.sql");
    sqlx::raw_sql(sql)
        .execute(pool)
        .await
        .map_err(|e| AppError::internal(format!("数据库迁移失败: {e}")))?;
    Ok(())
}

/// 执行 MySQL 初始迁移（migrations/mysql/0001_init.sql，可重复执行）。
///
/// sqlx 的 `raw_sql` 会将 SQL 按语句边界逐条派发执行；MySQL 无
/// `CREATE INDEX IF NOT EXISTS`，迁移文件的索引全部内联在 `CREATE TABLE` 中。
pub async fn migrate_mysql(pool: &sqlx::MySqlPool) -> Result<(), AppError> {
    let sql = include_str!("../migrations/mysql/0001_init.sql");
    sqlx::raw_sql(sql)
        .execute(pool)
        .await
        .map_err(|e| AppError::internal(format!("数据库迁移失败: {e}")))?;
    Ok(())
}

/// 打开 SQLite 数据库连接池并执行迁移。
pub async fn connect(url: &str) -> Result<DbPool, AppError> {
    let pool = sqlx::pool::PoolOptions::<sqlx::Sqlite>::new()
        .max_connections(8)
        .connect(url)
        .await
        .map_err(|e| AppError::internal(format!("数据库连接失败: {e}")))?;
    enable_wal(&pool).await?;
    migrate(&pool).await?;
    Ok(DbPool::Sqlite(pool))
}

/// 连接内存 SQLite（用于测试）。
pub async fn connect_in_memory() -> Result<DbPool, AppError> {
    let pool = sqlx::pool::PoolOptions::<sqlx::Sqlite>::new()
        .max_connections(4)
        .connect("sqlite::memory:")
        .await
        .map_err(|e| AppError::internal(format!("内存库连接失败: {e}")))?;
    migrate(&pool).await?;
    Ok(DbPool::Sqlite(pool))
}

/// 启用 WAL 模式与外键约束（仅 SQLite）。
async fn enable_wal(pool: &sqlx::SqlitePool) -> Result<(), AppError> {
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(pool)
        .await
        .map_err(AppError::from)?;
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(pool)
        .await
        .map_err(AppError::from)?;
    Ok(())
}

/// 检查数据库中是否已存在任何用户（用于首次初始化引导）。
pub async fn has_any_user(pool: &DbPool) -> Result<bool, AppError> {
    let row = DbQuery::new("SELECT COUNT(*) AS n FROM users")
        .fetch_one(pool)
        .await?;
    Ok(row.get_named_i64("n") > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 核心路径同一断言集（design D3）：建用户（自增回读）→ 建书签（布尔/时间/文本回读）
    /// → 标签上翻幂等 → 计数读取。三后端各自连接后调用，断言完全一致。
    /// 用户名带纳秒后缀：测试库可能与其他用例/端到端验证共享，避免唯一约束冲突。
    async fn assert_core_paths(pool: &DbPool) {
        let uname = format!(
            "alice{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        // 1. 建用户 + 自增回读
        let id: i64 = match pool {
            DbPool::Sqlite(_) | DbPool::MySql(_) => {

                q("INSERT INTO users (username, password_hash, is_admin, is_active, date_joined) VALUES (?, ?, ?, ?, ?)")
                    .bind(uname.as_str())
                    .bind("hash")
                    .bind(1i64)
                    .bind(1i64)
                    .bind("2026-01-01T00:00:00+00:00")
                    .execute(pool)
                    .await
                    .expect("插入用户失败")
                    .last_insert_rowid()
            }
            DbPool::Postgres(_) => {
                let row = q("INSERT INTO users (username, password_hash, is_admin, is_active, date_joined) VALUES (?, ?, TRUE, TRUE, ?) RETURNING id")
                    .bind(uname.as_str())
                    .bind("hash")
                    .bind("2026-01-01T00:00:00+00:00")
                    .fetch_one(pool)
                    .await
                    .expect("插入用户失败");
                row.get_i64(0)
            }
        };
        assert!(id > 0, "自增回读应得到正数主键");

        // 2. 建书签 + 布尔/时间/文本回读
        q("INSERT INTO bookmarks (user_id, url, url_hash, title, is_unread, is_shared, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(id)
            .bind("https://example.com/a")
            .bind("a1b2c3d4")
            .bind("示例")
            .bind(true)
            .bind(false)
            .bind("2026-01-02T03:04:05+00:00")
            .bind("2026-01-02T03:04:05+00:00")
            .execute(pool)
            .await
            .expect("插入书签失败");
        let row = q("SELECT b.id, b.title, b.is_unread, b.is_shared, b.created_at, u.date_joined FROM bookmarks b JOIN users u ON u.id = b.user_id WHERE b.user_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("回读书签失败");
        assert_eq!(row.get_named_string("title"), "示例");
        assert!(row.get_named_bool("is_unread"), "is_unread 应为真");
        assert!(!row.get_named_bool("is_shared"), "is_shared 应为假");
        assert!(row.get_named_i64("id") > 0);
        assert!(row.get_named_string("created_at").contains("2026"));
        assert!(row.get_named_string("date_joined").contains("2026"));

        // 3. 标签幂等（对齐 tags.rs ensure_tag 的业务模式：先查后插，重复查询/插入不产生重复行；
        //    SQLite 的 NOCASE 触发器会中止撞名插入，故不能直接重复 INSERT）。
        for _ in 0..2 {
            let existing = q("SELECT id FROM tags WHERE user_id = ? AND LOWER(name) = ?")
                .bind(id)
                .bind("rust")
                .fetch_optional(pool)
                .await
                .expect("查询标签失败");
            if existing.is_none() {
                q("INSERT INTO tags (user_id, name) VALUES (?, ?)")
                    .bind(id)
                    .bind("rust")
                    .execute(pool)
                    .await
                    .expect("标签插入失败");
            }
        }
        let n = q("SELECT COUNT(*) AS n FROM tags WHERE user_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("计数失败")
            .get_i64(0);
        assert_eq!(n, 1, "同一标签重复上翻应只有一行");
    }

    #[tokio::test]
    async fn core_paths_consistent_on_sqlite() {
        let pool = connect_in_memory().await.unwrap();
        assert_core_paths(&pool).await;
    }

    /// PostgreSQL 一致性测试：设置 LINKLIST_TEST_DATABASE_URL（指向已建库的 PG 实例）后随
    /// `cargo test` 自动执行；未设置时打印提示跳过（保持全绿）。
    #[tokio::test]
    async fn core_paths_consistent_on_postgres_when_configured() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let Some(url) = std::env::var("LINKLIST_TEST_DATABASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            eprintln!("跳过：未设置 LINKLIST_TEST_DATABASE_URL（PostgreSQL 一致性测试）");
            return;
        };
        let pool = sqlx::pool::PoolOptions::<sqlx::Postgres>::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("连接 PostgreSQL 测试实例失败");
        migrate_postgres(&pool).await.expect("PG 迁移失败");
        assert_core_paths(&DbPool::Postgres(pool)).await;
    }

    /// MySQL 一致性测试：设置 LINKLIST_TEST_MYSQL_URL（指向已建库的 MySQL 实例）后随
    /// `cargo test` 自动执行；未设置时打印提示跳过（保持全绿）。
    #[tokio::test]
    async fn core_paths_consistent_on_mysql_when_configured() {
        let _ = tracing_subscriber::fmt().with_test_writer().try_init();
        let Some(url) = std::env::var("LINKLIST_TEST_MYSQL_URL")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            eprintln!("跳过：未设置 LINKLIST_TEST_MYSQL_URL（MySQL 一致性测试）");
            return;
        };
        let pool = sqlx::pool::PoolOptions::<sqlx::MySql>::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("连接 MySQL 测试实例失败");
        migrate_mysql(&pool).await.expect("MySQL 迁移失败");
        assert_core_paths(&DbPool::MySql(pool)).await;
    }

    #[test]
    fn prefix_dispatch_decides_backend() {
        // 前缀分派规则（spec：连接串前缀决定后端）。
        assert!(is_postgres("postgres://u:p@h:5432/d"));
        assert!(is_postgres("postgresql://u:p@h:5432/d"));
        assert!(!is_postgres("mysql://u:p@h:3306/d"));
        assert!(is_mysql("mysql://u:p@h:3306/d"));
        assert!(is_mysql("mariadb://u:p@h:3306/d"));
        assert!(!is_mysql("postgres://u:p@h:5432/d"));
        assert!(!is_mysql("") && !is_postgres(""));
        assert!(!is_mysql("mssql://u:p@h") && !is_postgres("mssql://u:p@h"));
    }

    #[tokio::test]
    async fn unsupported_protocol_rejected() {
        // mssql:// 不退回 SQLite，直接报明确错误（退出码 2 的错误来源）。
        let err = connect_any("mssql://u:p@h:1433/db", "sqlite::memory:").await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(
            msg.contains("不支持的数据库连接串协议"),
            "错误信息应明确: {msg}"
        );
    }

    #[tokio::test]
    async fn file_db_wal_mode_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db_file = dir.path().join("t.sqlite3");
        let url = format!("sqlite://{}?mode=rwc", db_file.display());
        let pool = match connect(&url).await.unwrap() {
            DbPool::Sqlite(p) => p,
            _ => panic!(),
        };
        let row = sqlx::query("PRAGMA journal_mode;")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get(0);
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    #[tokio::test]
    async fn foreign_keys_enabled() {
        let pool = match connect_in_memory().await.unwrap() {
            DbPool::Sqlite(p) => p,
            _ => panic!(),
        };
        let row = sqlx::query("PRAGMA foreign_keys;")
            .fetch_one(&pool)
            .await
            .unwrap();
        let fk: i64 = row.get(0);
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn migrations_create_expected_tables() {
        let pool = match connect_in_memory().await.unwrap() {
            DbPool::Sqlite(p) => p,
            _ => panic!(),
        };
        for t in [
            "users",
            "user_profiles",
            "api_tokens",
            "feed_tokens",
            "bookmarks",
            "tags",
            "bookmark_tags",
            "bookmark_assets",
            "bundles",
            "capture_tasks",
            "audit_logs",
            "settings",
            "bookmarks_fts",
        ] {
            // 表名来自代码内白名单常量，逐表静态查询（sqlx 0.9 动态 SQL 审计）。
            let sql = match t {
                "users" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='users'"
                }
                "user_profiles" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='user_profiles'"
                }
                "api_tokens" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='api_tokens'"
                }
                "feed_tokens" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='feed_tokens'"
                }
                "bookmarks" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='bookmarks'"
                }
                "tags" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='tags'"
                }
                "bookmark_tags" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='bookmark_tags'"
                }
                "bookmark_assets" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='bookmark_assets'"
                }
                "bundles" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='bundles'"
                }
                "capture_tasks" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='capture_tasks'"
                }
                "audit_logs" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='audit_logs'"
                }
                "settings" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='settings'"
                }
                "bookmarks_fts" => {
                    "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table','trigger') AND name='bookmarks_fts'"
                }
                _ => unreachable!("未知的表名校验项"),
            };
            let n: i64 = sqlx::query_scalar(sql).fetch_one(&pool).await.unwrap();
            assert_eq!(n, 1, "缺少表/触发器 {t}");
        }
    }

    #[tokio::test]
    async fn fts_trigger_sync() {
        let pool = match connect_in_memory().await.unwrap() {
            DbPool::Sqlite(p) => p,
            _ => panic!(),
        };
        sqlx::query("INSERT INTO users (username, password_hash, date_joined) VALUES ('u','x','2025-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO bookmarks (user_id, url, url_hash, title, created_at, updated_at) VALUES (1,'https://a','h1','Rust Book','2025-01-01T00:00:00Z','2025-01-01T00:00:00Z')")
            .execute(&pool).await.unwrap();
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bookmarks_fts WHERE bookmarks_fts MATCH 'rust'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1);
        sqlx::query("UPDATE bookmarks SET title='Tokio Guide' WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM bookmarks_fts WHERE bookmarks_fts MATCH 'tokio'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 1);
        sqlx::query("DELETE FROM bookmarks WHERE id=1")
            .execute(&pool)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn in_memory_db_migrates_and_has_no_user() {
        let pool = connect_in_memory().await.unwrap();
        assert!(!has_any_user(&pool).await.unwrap());
    }

    #[test]
    fn postgres_url_detection() {
        assert!(is_postgres("postgres://u:p@h/db"));
        assert!(is_postgres("postgresql://u:p@h/db"));
        assert!(!is_postgres("sqlite://data.db"));
        assert!(!is_postgres(""));
    }

    #[test]
    fn db_value_conversions() {
        assert!(matches!(DbValue::from(7i64), DbValue::I64(7)));
        assert!(matches!(DbValue::from("x"), DbValue::Text(_)));
        assert!(matches!(DbValue::from(true), DbValue::Bool(true)));
        assert!(matches!(DbValue::from(None::<i64>), DbValue::Null));
    }
}
