//! `Executor` trait — the seam between the dialect-aware SQL we emit and the
//! sqlx pool that runs it. Builders are kept executor-agnostic so they stay
//! testable without a database (Dependency Inversion Principle).
//!
//! Concrete impls live behind feature flags (`sqlite`, `postgres`, `mysql`).
//! Builders call `Executor::fetch_*` / `execute` after rendering via `ToSql`.

use async_trait::async_trait;
use serde_json::Value as JsonValue;

use crate::dialect::Dialect;
use crate::error::Result;
use crate::value::Value;

#[async_trait]
pub trait Executor: Send + Sync {
    fn dialect(&self) -> Dialect;

    async fn fetch_all_raw(&self, sql: &str, params: &[Value]) -> Result<Vec<JsonValue>>;

    async fn fetch_optional_raw(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<Option<JsonValue>>;

    async fn execute_raw(&self, sql: &str, params: &[Value]) -> Result<u64>;
}

// ---------------------------------------------------------------------------
// sqlx::Pool<Sqlite> impl
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
mod sqlite_impl {
    use super::*;
    use sqlx::{Column, Row};

    fn bind_params<'q>(
        mut q: sqlx::query::Query<
            'q,
            sqlx::Sqlite,
            sqlx::sqlite::SqliteArguments<'q>,
        >,
        params: &'q [Value],
    ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(*b),
                Value::Int(i) => q.bind(*i),
                Value::Real(f) => q.bind(*f),
                Value::Text(s) => q.bind(s.clone()),
                Value::Json(j) => q.bind(j.to_string()),
            };
        }
        q
    }

    fn row_to_json(row: &sqlx::sqlite::SqliteRow) -> JsonValue {
        // SQLite is dynamically typed and computed columns (COUNT(*), SUM(x))
        // often carry no type info — so probe in order rather than trust the
        // declared type. i64 first so integer-looking results come out typed
        // correctly; fall through to f64 / bool / String / Null.
        let mut map = serde_json::Map::new();
        for col in row.columns() {
            let name = col.name();
            let val: JsonValue = if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(name) {
                JsonValue::from(v)
            } else if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(name) {
                serde_json::Number::from_f64(v)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null)
            } else if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(name) {
                JsonValue::from(v)
            } else if let Ok(Some(v)) = row.try_get::<Option<String>, _>(name) {
                JsonValue::from(v)
            } else {
                JsonValue::Null
            };
            map.insert(name.to_owned(), val);
        }
        JsonValue::Object(map)
    }

    #[async_trait]
    impl Executor for sqlx::SqlitePool {
        fn dialect(&self) -> Dialect {
            Dialect::Sqlite
        }

        async fn fetch_all_raw(&self, sql: &str, params: &[Value]) -> Result<Vec<JsonValue>> {
            let q = bind_params(sqlx::query(sql), params);
            let rows = q.fetch_all(self).await?;
            Ok(rows.iter().map(row_to_json).collect())
        }

        async fn fetch_optional_raw(
            &self,
            sql: &str,
            params: &[Value],
        ) -> Result<Option<JsonValue>> {
            let q = bind_params(sqlx::query(sql), params);
            let row = q.fetch_optional(self).await?;
            Ok(row.as_ref().map(row_to_json))
        }

        async fn execute_raw(&self, sql: &str, params: &[Value]) -> Result<u64> {
            let q = bind_params(sqlx::query(sql), params);
            let r = q.execute(self).await?;
            Ok(r.rows_affected())
        }
    }
}

// ---------------------------------------------------------------------------
// sqlx::Pool<Postgres> impl
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
mod postgres_impl {
    use super::*;
    use sqlx::{Column, Row};

    fn bind_params<'q>(
        mut q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
        params: &'q [Value],
    ) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(*b),
                Value::Int(i) => q.bind(*i),
                Value::Real(f) => q.bind(*f),
                Value::Text(s) => q.bind(s.clone()),
                // sqlx supports JsonValue natively for Postgres when the
                // `json` feature is on (it is).
                Value::Json(j) => q.bind(j.clone()),
            };
        }
        q
    }

    fn row_to_json(row: &sqlx::postgres::PgRow) -> JsonValue {
        let mut map = serde_json::Map::new();
        for col in row.columns() {
            let name = col.name();
            let val: JsonValue = if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(name) {
                JsonValue::from(v)
            } else if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(name) {
                JsonValue::from(v as i64)
            } else if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(name) {
                serde_json::Number::from_f64(v)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null)
            } else if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(name) {
                JsonValue::from(v)
            } else if let Ok(Some(v)) = row.try_get::<Option<JsonValue>, _>(name) {
                v
            } else if let Ok(Some(v)) = row.try_get::<Option<String>, _>(name) {
                JsonValue::from(v)
            } else {
                JsonValue::Null
            };
            map.insert(name.to_owned(), val);
        }
        JsonValue::Object(map)
    }

    #[async_trait]
    impl Executor for sqlx::PgPool {
        fn dialect(&self) -> Dialect {
            Dialect::Postgres
        }

        async fn fetch_all_raw(&self, sql: &str, params: &[Value]) -> Result<Vec<JsonValue>> {
            let q = bind_params(sqlx::query(sql), params);
            let rows = q.fetch_all(self).await?;
            Ok(rows.iter().map(row_to_json).collect())
        }

        async fn fetch_optional_raw(
            &self,
            sql: &str,
            params: &[Value],
        ) -> Result<Option<JsonValue>> {
            let q = bind_params(sqlx::query(sql), params);
            let row = q.fetch_optional(self).await?;
            Ok(row.as_ref().map(row_to_json))
        }

        async fn execute_raw(&self, sql: &str, params: &[Value]) -> Result<u64> {
            let q = bind_params(sqlx::query(sql), params);
            let r = q.execute(self).await?;
            Ok(r.rows_affected())
        }
    }
}

// ---------------------------------------------------------------------------
// sqlx::Pool<MySql> impl
// ---------------------------------------------------------------------------

#[cfg(feature = "mysql")]
mod mysql_impl {
    use super::*;
    use sqlx::{Column, Row};

    fn bind_params<'q>(
        mut q: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
        params: &'q [Value],
    ) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(*b),
                Value::Int(i) => q.bind(*i),
                Value::Real(f) => q.bind(*f),
                Value::Text(s) => q.bind(s.clone()),
                // MySQL JSON binds reliably via stringified JSON; the server
                // validates and stores as JSON column type.
                Value::Json(j) => q.bind(j.to_string()),
            };
        }
        q
    }

    fn row_to_json(row: &sqlx::mysql::MySqlRow) -> JsonValue {
        let mut map = serde_json::Map::new();
        for col in row.columns() {
            let name = col.name();
            let val: JsonValue = if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(name) {
                JsonValue::from(v)
            } else if let Ok(Some(v)) = row.try_get::<Option<i32>, _>(name) {
                JsonValue::from(v as i64)
            } else if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(name) {
                serde_json::Number::from_f64(v)
                    .map(JsonValue::Number)
                    .unwrap_or(JsonValue::Null)
            } else if let Ok(Some(v)) = row.try_get::<Option<bool>, _>(name) {
                JsonValue::from(v)
            } else if let Ok(Some(v)) = row.try_get::<Option<String>, _>(name) {
                JsonValue::from(v)
            } else {
                JsonValue::Null
            };
            map.insert(name.to_owned(), val);
        }
        JsonValue::Object(map)
    }

    #[async_trait]
    impl Executor for sqlx::MySqlPool {
        fn dialect(&self) -> Dialect {
            Dialect::MySql
        }

        async fn fetch_all_raw(&self, sql: &str, params: &[Value]) -> Result<Vec<JsonValue>> {
            let q = bind_params(sqlx::query(sql), params);
            let rows = q.fetch_all(self).await?;
            Ok(rows.iter().map(row_to_json).collect())
        }

        async fn fetch_optional_raw(
            &self,
            sql: &str,
            params: &[Value],
        ) -> Result<Option<JsonValue>> {
            let q = bind_params(sqlx::query(sql), params);
            let row = q.fetch_optional(self).await?;
            Ok(row.as_ref().map(row_to_json))
        }

        async fn execute_raw(&self, sql: &str, params: &[Value]) -> Result<u64> {
            let q = bind_params(sqlx::query(sql), params);
            let r = q.execute(self).await?;
            Ok(r.rows_affected())
        }
    }
}

// ---------------------------------------------------------------------------
// Execution helpers — implemented as inherent methods on builders via this
// blanket so we don't repeat the body in Select/Update/Delete/Insert/Raw.
// ---------------------------------------------------------------------------

use crate::sql::ToSql;
use serde::de::DeserializeOwned;

pub async fn fetch_all<B, E, T>(builder: &B, exec: &E) -> Result<Vec<T>>
where
    B: ToSql + ?Sized,
    E: Executor,
    T: DeserializeOwned,
{
    let (sql, params) = builder.to_sql(exec.dialect());
    let rows = exec.fetch_all_raw(&sql, &params).await?;
    rows.into_iter()
        .map(|v| serde_json::from_value::<T>(v).map_err(|e| crate::error::Error::Builder(e.to_string())))
        .collect()
}

pub async fn fetch_optional<B, E, T>(builder: &B, exec: &E) -> Result<Option<T>>
where
    B: ToSql + ?Sized,
    E: Executor,
    T: DeserializeOwned,
{
    let (sql, params) = builder.to_sql(exec.dialect());
    let row = exec.fetch_optional_raw(&sql, &params).await?;
    match row {
        Some(v) => Ok(Some(
            serde_json::from_value::<T>(v)
                .map_err(|e| crate::error::Error::Builder(e.to_string()))?,
        )),
        None => Ok(None),
    }
}

pub async fn fetch_one<B, E, T>(builder: &B, exec: &E) -> Result<T>
where
    B: ToSql + ?Sized,
    E: Executor,
    T: DeserializeOwned,
{
    match fetch_optional::<B, E, T>(builder, exec).await? {
        Some(v) => Ok(v),
        None => Err(crate::error::Error::Builder("no rows returned".into())),
    }
}

pub async fn execute<B, E>(builder: &B, exec: &E) -> Result<u64>
where
    B: ToSql + ?Sized,
    E: Executor,
{
    let (sql, params) = builder.to_sql(exec.dialect());
    exec.execute_raw(&sql, &params).await
}
