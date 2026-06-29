//! Native fast-path execution: skips the `serde_json` round-trip and hands the
//! built `(sql, params)` straight to `sqlx::query_as<R>`, which decodes via the
//! `FromRow` trait directly off the wire buffer.
//!
//! Use these methods on hot paths where you have a typed struct and a concrete
//! `sqlx::Pool<DB>`. The generic `Execute` trait (JSON path) stays available
//! for dynamic-schema and polyglot scenarios.
//!
//! Per-backend trait so method resolution disambiguates by pool type:
//! `User::find(1).fetch_one_as::<User>(&pg_pool)` — PgExecute is picked.
//! `User::find(1).fetch_one_as::<User>(&sqlite_pool)` — LiteExecute is picked.

use crate::dialect::Dialect;
use crate::error::Result;
use crate::sql::ToSql;
use crate::value::Value;

// ---------------------------------------------------------------------------
// Postgres
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub mod pg {
    use super::*;
    use async_trait::async_trait;
    use futures_core::stream::BoxStream;
    use futures_util::TryStreamExt;
    use sqlx::{FromRow, PgPool, Postgres, postgres::PgArguments, postgres::PgRow};

    fn bind_query_as<'q, R>(
        mut q: sqlx::query::QueryAs<'q, Postgres, R, PgArguments>,
        params: Vec<Value>,
    ) -> sqlx::query::QueryAs<'q, Postgres, R, PgArguments> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(b),
                Value::Int(i) => q.bind(i),
                Value::Real(f) => q.bind(f),
                Value::Text(s) => q.bind(s),
                Value::Json(j) => q.bind(j),
            };
        }
        q
    }

    fn bind_query<'q>(
        mut q: sqlx::query::Query<'q, Postgres, PgArguments>,
        params: Vec<Value>,
    ) -> sqlx::query::Query<'q, Postgres, PgArguments> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(b),
                Value::Int(i) => q.bind(i),
                Value::Real(f) => q.bind(f),
                Value::Text(s) => q.bind(s),
                Value::Json(j) => q.bind(j),
            };
        }
        q
    }

    #[async_trait]
    pub trait PgExecute: ToSql + Sync {
        // -- pool path --------------------------------------------------
        async fn fetch_all_as<R>(&self, pool: &PgPool) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
        async fn fetch_one_as<R>(&self, pool: &PgPool) -> Result<R>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
        async fn fetch_optional_as<R>(&self, pool: &PgPool) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
        async fn execute_on(&self, pool: &PgPool) -> Result<u64>;

        // -- transaction path ------------------------------------------
        async fn fetch_all_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
        async fn fetch_one_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<R>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
        async fn fetch_optional_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
        async fn execute_in(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<u64>;

        // -- streaming -------------------------------------------------
        fn fetch_stream_as<'a, R>(
            &'a self,
            pool: &'a PgPool,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;

        fn fetch_stream_as_in<'a, R>(
            &'a self,
            tx: &'a mut sqlx::Transaction<'_, Postgres>,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static;
    }

    #[async_trait]
    impl<B: ToSql + Sync + ?Sized> PgExecute for B {
        async fn fetch_all_as<R>(&self, pool: &PgPool) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_all(pool).await?)
        }

        async fn fetch_one_as<R>(&self, pool: &PgPool) -> Result<R>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_one(pool).await?)
        }

        async fn fetch_optional_as<R>(&self, pool: &PgPool) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_optional(pool).await?)
        }

        async fn execute_on(&self, pool: &PgPool) -> Result<u64> {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query(sqlx::query(&sql), params);
            Ok(q.execute(pool).await?.rows_affected())
        }

        async fn fetch_all_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_all(&mut **tx).await?)
        }

        async fn fetch_one_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<R>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_one(&mut **tx).await?)
        }

        async fn fetch_optional_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_optional(&mut **tx).await?)
        }

        async fn execute_in(
            &self,
            tx: &mut sqlx::Transaction<'_, Postgres>,
        ) -> Result<u64> {
            let (sql, params) = self.to_sql(Dialect::Postgres);
            let q = bind_query(sqlx::query(&sql), params);
            Ok(q.execute(&mut **tx).await?.rows_affected())
        }

        fn fetch_stream_as<'a, R>(
            &'a self,
            pool: &'a PgPool,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            Box::pin(async_stream::try_stream! {
                let (sql, params) = self.to_sql(Dialect::Postgres);
                let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
                let mut s = q.fetch(pool);
                while let Some(row) = s.try_next().await? {
                    yield row;
                }
            })
        }

        fn fetch_stream_as_in<'a, R>(
            &'a self,
            tx: &'a mut sqlx::Transaction<'_, Postgres>,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, PgRow> + Send + Unpin + 'static,
        {
            Box::pin(async_stream::try_stream! {
                let (sql, params) = self.to_sql(Dialect::Postgres);
                let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
                let mut s = q.fetch(&mut **tx);
                while let Some(row) = s.try_next().await? {
                    yield row;
                }
            })
        }
    }
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

#[cfg(feature = "mysql")]
pub mod my {
    use super::*;
    use async_trait::async_trait;
    use futures_core::stream::BoxStream;
    use futures_util::TryStreamExt;
    use sqlx::{FromRow, MySql, MySqlPool, mysql::MySqlArguments, mysql::MySqlRow};

    fn bind_query_as<'q, R>(
        mut q: sqlx::query::QueryAs<'q, MySql, R, MySqlArguments>,
        params: Vec<Value>,
    ) -> sqlx::query::QueryAs<'q, MySql, R, MySqlArguments> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(b),
                Value::Int(i) => q.bind(i),
                Value::Real(f) => q.bind(f),
                Value::Text(s) => q.bind(s),
                Value::Json(j) => q.bind(j.to_string()),
            };
        }
        q
    }

    fn bind_query<'q>(
        mut q: sqlx::query::Query<'q, MySql, MySqlArguments>,
        params: Vec<Value>,
    ) -> sqlx::query::Query<'q, MySql, MySqlArguments> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(b),
                Value::Int(i) => q.bind(i),
                Value::Real(f) => q.bind(f),
                Value::Text(s) => q.bind(s),
                Value::Json(j) => q.bind(j.to_string()),
            };
        }
        q
    }

    #[async_trait]
    pub trait MyExecute: ToSql + Sync {
        async fn fetch_all_as<R>(&self, pool: &MySqlPool) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
        async fn fetch_one_as<R>(&self, pool: &MySqlPool) -> Result<R>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
        async fn fetch_optional_as<R>(&self, pool: &MySqlPool) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
        async fn execute_on(&self, pool: &MySqlPool) -> Result<u64>;

        async fn fetch_all_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
        async fn fetch_one_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<R>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
        async fn fetch_optional_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
        async fn execute_in(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<u64>;

        fn fetch_stream_as<'a, R>(
            &'a self,
            pool: &'a MySqlPool,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;

        fn fetch_stream_as_in<'a, R>(
            &'a self,
            tx: &'a mut sqlx::Transaction<'_, MySql>,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static;
    }

    #[async_trait]
    impl<B: ToSql + Sync + ?Sized> MyExecute for B {
        async fn fetch_all_as<R>(&self, pool: &MySqlPool) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_all(pool).await?)
        }

        async fn fetch_one_as<R>(&self, pool: &MySqlPool) -> Result<R>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_one(pool).await?)
        }

        async fn fetch_optional_as<R>(&self, pool: &MySqlPool) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_optional(pool).await?)
        }

        async fn execute_on(&self, pool: &MySqlPool) -> Result<u64> {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query(sqlx::query(&sql), params);
            Ok(q.execute(pool).await?.rows_affected())
        }

        async fn fetch_all_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_all(&mut **tx).await?)
        }

        async fn fetch_one_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<R>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_one(&mut **tx).await?)
        }

        async fn fetch_optional_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_optional(&mut **tx).await?)
        }

        async fn execute_in(
            &self,
            tx: &mut sqlx::Transaction<'_, MySql>,
        ) -> Result<u64> {
            let (sql, params) = self.to_sql(Dialect::MySql);
            let q = bind_query(sqlx::query(&sql), params);
            Ok(q.execute(&mut **tx).await?.rows_affected())
        }

        fn fetch_stream_as<'a, R>(
            &'a self,
            pool: &'a MySqlPool,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            Box::pin(async_stream::try_stream! {
                let (sql, params) = self.to_sql(Dialect::MySql);
                let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
                let mut s = q.fetch(pool);
                while let Some(row) = s.try_next().await? {
                    yield row;
                }
            })
        }

        fn fetch_stream_as_in<'a, R>(
            &'a self,
            tx: &'a mut sqlx::Transaction<'_, MySql>,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, MySqlRow> + Send + Unpin + 'static,
        {
            Box::pin(async_stream::try_stream! {
                let (sql, params) = self.to_sql(Dialect::MySql);
                let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
                let mut s = q.fetch(&mut **tx);
                while let Some(row) = s.try_next().await? {
                    yield row;
                }
            })
        }
    }
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
pub mod lite {
    use super::*;
    use async_trait::async_trait;
    use futures_core::stream::BoxStream;
    use futures_util::TryStreamExt;
    use sqlx::{FromRow, Sqlite, SqlitePool, sqlite::SqliteArguments, sqlite::SqliteRow};

    fn bind_query_as<'q, R>(
        mut q: sqlx::query::QueryAs<'q, Sqlite, R, SqliteArguments<'q>>,
        params: Vec<Value>,
    ) -> sqlx::query::QueryAs<'q, Sqlite, R, SqliteArguments<'q>> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(b),
                Value::Int(i) => q.bind(i),
                Value::Real(f) => q.bind(f),
                Value::Text(s) => q.bind(s),
                Value::Json(j) => q.bind(j.to_string()),
            };
        }
        q
    }

    fn bind_query<'q>(
        mut q: sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>>,
        params: Vec<Value>,
    ) -> sqlx::query::Query<'q, Sqlite, SqliteArguments<'q>> {
        for p in params {
            q = match p {
                Value::Null => q.bind(Option::<String>::None),
                Value::Bool(b) => q.bind(b),
                Value::Int(i) => q.bind(i),
                Value::Real(f) => q.bind(f),
                Value::Text(s) => q.bind(s),
                Value::Json(j) => q.bind(j.to_string()),
            };
        }
        q
    }

    #[async_trait]
    pub trait LiteExecute: ToSql + Sync {
        async fn fetch_all_as<R>(&self, pool: &SqlitePool) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
        async fn fetch_one_as<R>(&self, pool: &SqlitePool) -> Result<R>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
        async fn fetch_optional_as<R>(&self, pool: &SqlitePool) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
        async fn execute_on(&self, pool: &SqlitePool) -> Result<u64>;

        async fn fetch_all_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
        async fn fetch_one_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<R>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
        async fn fetch_optional_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
        async fn execute_in(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<u64>;

        fn fetch_stream_as<'a, R>(
            &'a self,
            pool: &'a SqlitePool,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;

        fn fetch_stream_as_in<'a, R>(
            &'a self,
            tx: &'a mut sqlx::Transaction<'_, Sqlite>,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static;
    }

    #[async_trait]
    impl<B: ToSql + Sync + ?Sized> LiteExecute for B {
        async fn fetch_all_as<R>(&self, pool: &SqlitePool) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_all(pool).await?)
        }

        async fn fetch_one_as<R>(&self, pool: &SqlitePool) -> Result<R>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_one(pool).await?)
        }

        async fn fetch_optional_as<R>(&self, pool: &SqlitePool) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_optional(pool).await?)
        }

        async fn execute_on(&self, pool: &SqlitePool) -> Result<u64> {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query(sqlx::query(&sql), params);
            Ok(q.execute(pool).await?.rows_affected())
        }

        async fn fetch_all_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<Vec<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_all(&mut **tx).await?)
        }

        async fn fetch_one_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<R>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_one(&mut **tx).await?)
        }

        async fn fetch_optional_as_in<R>(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<Option<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
            Ok(q.fetch_optional(&mut **tx).await?)
        }

        async fn execute_in(
            &self,
            tx: &mut sqlx::Transaction<'_, Sqlite>,
        ) -> Result<u64> {
            let (sql, params) = self.to_sql(Dialect::Sqlite);
            let q = bind_query(sqlx::query(&sql), params);
            Ok(q.execute(&mut **tx).await?.rows_affected())
        }

        fn fetch_stream_as<'a, R>(
            &'a self,
            pool: &'a SqlitePool,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            Box::pin(async_stream::try_stream! {
                let (sql, params) = self.to_sql(Dialect::Sqlite);
                let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
                let mut s = q.fetch(pool);
                while let Some(row) = s.try_next().await? {
                    yield row;
                }
            })
        }

        fn fetch_stream_as_in<'a, R>(
            &'a self,
            tx: &'a mut sqlx::Transaction<'_, Sqlite>,
        ) -> BoxStream<'a, Result<R>>
        where
            R: for<'r> FromRow<'r, SqliteRow> + Send + Unpin + 'static,
        {
            Box::pin(async_stream::try_stream! {
                let (sql, params) = self.to_sql(Dialect::Sqlite);
                let q = bind_query_as(sqlx::query_as::<_, R>(&sql), params);
                let mut s = q.fetch(&mut **tx);
                while let Some(row) = s.try_next().await? {
                    yield row;
                }
            })
        }
    }
}
