# ormx-rust-client

Thin Rust query DSL on top of [sqlx](https://github.com/launchbadge/sqlx). Typed builder, dialect-aware placeholders, optional migrations and schema codegen behind feature flags.

```rust
use ormx::{Model, eq, desc};
use sqlx::postgres::PgPoolOptions;

#[derive(sqlx::FromRow)]
struct User { id: i64, name: String, email: String, age: i32 }
struct Users;
impl Model for Users { fn table_name() -> &'static str { "users" } }

let pool = PgPoolOptions::new().connect("postgres://localhost/app").await?;

// Typed query DSL → real SQL + params
let adults: Vec<User> = Users::all()
    .where_("age", eq(18))
    .order_by("created_at", desc())
    .limit(10)
    .fetch_all_as::<User>(&pool).await?;

// SQL-only — no DB required
let (sql, params) = Users::all()
    .where_("email", eq("a@b.com"))
    .limit(1)
    .to_sql(ormx::Dialect::Postgres);
```

## What it covers

- Builder: `where/whereIn/whereBetween/whereNull/whereRaw`, `orderBy`, `limit/offset/paginate`, `join/leftJoin`, `groupBy/having`, `select/distinct`
- Writes: `insert/insert_many/update/delete/upsert`
- Streaming: `fetch_stream_as<R>` — memory-bounded iteration via sqlx's wire-level row stream.

  ```rust
  use futures_util::StreamExt;
  let q = Users::all().where_("active", eq(true));
  let mut stream = q.fetch_stream_as::<User>(&pool);
  while let Some(row) = stream.next().await {
      let user = row?;
      send_welcome(user).await?;
  }
  ```
  Drop the stream (or `break`) to release the underlying pool connection.
- Dialects: Postgres family (PG, Cockroach, Yugabyte, Neon), MySQL family (MySQL, MariaDB, TiDB, PlanetScale), SQLite family (SQLite, libsql), Oracle, MS SQL, DB2 — placeholder syntax and `RETURNING` support routed per family
- Native execution: `PgExecute / MyExecute / LiteExecute` traits hand the built query straight to `sqlx::query_as<R>` for zero-overhead `FromRow` decode
- Generic execution: `Execute` trait that round-trips through `serde_json::Value` — slower (~6×) but works with any executor and any `Deserialize` type

## Optional features

| Feature | What it gives you |
|---|---|
| `postgres` | Activates Postgres-specific paths (`PgExecute`, `RETURNING *`, …) |
| `mysql` | Activates MySQL paths (`MyExecute`) |
| `sqlite` (default) | Activates SQLite paths (`LiteExecute`) |
| `migrations` | `ormx::migrate` — sql-file migrations, drift detection, transactional apply |
| `codegen` | `ormx::codegen` — introspect `information_schema`, emit Rust/Python/Node/Go/Ruby/Java models |

## Performance

```
fetch_one  (1 row)    native ≈ 1.02× raw sqlx
fetch_all  (100 rows) native ≈ 1.14× raw sqlx
generic (JSON path)         ≈ 1.3–2.8× the native path
```

See `benches/perf.rs` and `benches/decomposed.rs` (the latter splits wire cost from BEGIN/COMMIT durability cost for clean reporting).

## Serverless Postgres

ormx works with every managed Postgres that speaks the wire protocol —
Neon, Supabase, CockroachDB Serverless, AWS RDS/Aurora, Cloud SQL,
Azure, Render, Railway, Xata, Fly. See [SERVERLESS.md](SERVERLESS.md)
for the compatibility matrix + dialect-routing notes.

## Status

Pre-1.0. Public surface is stable for what's documented; new features land behind feature flags. Pin a minor version.

## License

Apache-2.0
