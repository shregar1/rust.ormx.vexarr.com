//! # ormx — thin Rust query DSL on top of sqlx
//!
//! Goals: cover ~90% of CRUD without becoming a full ORM. No lazy loading,
//! no auto-relations, no dirty tracking, no schema reflection. Just a typed
//! builder that emits parameterised SQL, plus a raw escape hatch.
//!
//! ## Execution paths
//!
//! Two ways to run a built query — pick by import:
//!
//! * **Native (`PgExecute` / `MyExecute` / `LiteExecute`)** — `fetch_all_as<R>`
//!   / `fetch_one_as<R>` / `fetch_optional_as<R>`. Hands the query straight to
//!   `sqlx::query_as<R>` and decodes via `FromRow`. **At native sqlx speed**
//!   (~1.02× for fetch_one, ~1.14× for fetch_all of 100 rows in the bundled
//!   bench against in-memory SQLite). Use this on hot paths.
//!
//! * **Generic (`Execute`)** — `fetch_all<T: DeserializeOwned>` etc. Goes
//!   through `serde_json` so it works with any `Executor` impl and any
//!   `Deserialize` type. ~1.3–2.8× the native path. Use when the schema is
//!   dynamic, when polyglot-decoding the same row in multiple places, or for
//!   debugging.
//!
//! ```ignore
//! use ormx::{Model, eq, gt, desc};
//!
//! struct User;
//! impl Model for User { fn table_name() -> &'static str { "users" } }
//!
//! // pure SQL — no DB required
//! let (sql, params) = User::query()
//!     .select(&["id", "email"])
//!     .where_("email", eq("a@b.com"))
//!     .order_by("created_at", desc())
//!     .limit(1)
//!     .to_sql(ormx::Dialect::Postgres);
//!
//! // with a sqlx pool
//! // let user: User = User::find(1).fetch_one(&pool).await?;
//! ```
//!
//! ## Schema migrations
//!
//! Behind the `migrations` Cargo feature (Postgres only in Phase 1):
//! a directory of `<version>__<name>.up.sql` (+ optional `.down.sql`)
//! files, transactional apply, checksum-tracked drift detection.
//! See [`migrate`] for the full surface.
//!
//! ## Codegen
//!
//! Behind the `codegen` Cargo feature (Postgres only in Phase 1):
//! introspect `information_schema` and emit a Rust source file with
//! a `struct` per table, derived `sqlx::FromRow`, and an `impl Model`
//! block. See [`codegen`] for the full surface.

mod delete;
mod dialect;
mod error;
mod executor;
mod expr;
mod insert;
#[cfg(feature = "codegen")]
pub mod codegen;
#[cfg(feature = "migrations")]
pub mod migrate;
mod model;
mod native;
mod raw;
mod select;
mod sql;
mod update;
mod value;

// -- Public surface --------------------------------------------------------

pub use delete::Delete;
pub use dialect::{Dialect, Family};
pub use error::{Error, Result};
pub use executor::Executor;
pub use expr::{
    Cond, Direction, Op, and, asc, between, desc, eq, gt, gte, ilike, in_, is_null, like, lt,
    lte, ne, not, not_in, not_null, or, pred,
};
pub use insert::{Insert, InsertOnConflict};
pub use model::Model;
pub use raw::{Raw, raw};
pub use select::Select;
pub use sql::ToSql;
pub use update::Update;
pub use value::Value;

#[cfg(feature = "postgres")]
pub use native::pg::PgExecute;
#[cfg(feature = "mysql")]
pub use native::my::MyExecute;
#[cfg(feature = "sqlite")]
pub use native::lite::LiteExecute;

// Execution helpers exposed as inherent-style methods on the builders.
// Implemented via a trait so we DRY the body once.

use serde::de::DeserializeOwned;

#[async_trait::async_trait]
pub trait Execute: ToSql + Sync {
    async fn fetch_all<E, T>(&self, exec: &E) -> Result<Vec<T>>
    where
        E: Executor,
        T: DeserializeOwned + Send,
    {
        executor::fetch_all(self, exec).await
    }

    async fn fetch_optional<E, T>(&self, exec: &E) -> Result<Option<T>>
    where
        E: Executor,
        T: DeserializeOwned + Send,
    {
        executor::fetch_optional(self, exec).await
    }

    async fn fetch_one<E, T>(&self, exec: &E) -> Result<T>
    where
        E: Executor,
        T: DeserializeOwned + Send,
    {
        executor::fetch_one(self, exec).await
    }

    async fn execute<E>(&self, exec: &E) -> Result<u64>
    where
        E: Executor,
    {
        executor::execute(self, exec).await
    }
}

impl<B: ToSql + Sync> Execute for B {}
