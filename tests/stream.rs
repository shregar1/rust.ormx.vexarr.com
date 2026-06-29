//! Streaming pattern — demonstrates `fetch_stream_as` against real PG.
//!
//! Memory-bounded result iteration: sqlx returns a `BoxStream` over the
//! pg wire, yielding rows as DataRow messages come off the socket. Use
//! `StreamExt::next().await` (or `try_for_each`, etc.) to consume.
//!
//! Skipped without `ORMX_TEST_PG_DSN`.

#![cfg(feature = "postgres")]

use futures_util::StreamExt;
use ormx::{Model, PgExecute};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn dsn() -> Option<String> {
    std::env::var("ORMX_TEST_PG_DSN").ok()
}

#[derive(Debug, sqlx::FromRow)]
struct StreamUser {
    id: i64,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    email: String,
    #[allow(dead_code)]
    age: i32,
}

struct StreamUsers;
impl Model for StreamUsers {
    fn table_name() -> &'static str { "stream_users_rust" }
}

async fn fresh_schema(name: &str) -> (PgPool, String) {
    let dsn = dsn().expect("ORMX_TEST_PG_DSN must be set");
    let schema = format!("rs_stream_{name}_{}", std::process::id());
    let admin = PgPoolOptions::new().max_connections(1).connect(&dsn).await.unwrap();
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).execute(&admin).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&admin).await.unwrap();
    admin.close().await;
    let s = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _| {
            let s = s.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {s}"))
                    .execute(&mut *conn).await?;
                Ok(())
            })
        })
        .connect(&dsn).await.unwrap();
    (pool, schema)
}

async fn drop_schema(pool: &PgPool, schema: &str) {
    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE")).execute(pool).await;
}

async fn seed(pool: &PgPool, n: usize) {
    sqlx::query(
        "CREATE TABLE stream_users_rust (
            id BIGSERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            email TEXT UNIQUE NOT NULL,
            age INT NOT NULL
        )"
    ).execute(pool).await.unwrap();
    let mut sql = String::from("INSERT INTO stream_users_rust (name, email, age) VALUES ");
    let mut args = Vec::with_capacity(n * 3);
    let mut params = String::new();
    let mut i = 1;
    for r in 0..n {
        if r > 0 { params.push(','); }
        params.push_str(&format!("(${},${},${})", i, i + 1, i + 2));
        args.push(format!("U{r}"));
        args.push(format!("s{r}@x.com"));
        i += 3;
    }
    sql.push_str(&params);
    let mut q = sqlx::query(&sql);
    for r in 0..n {
        q = q.bind(args.remove(0)).bind(args.remove(0)).bind(20i32 + (r as i32 % 30));
    }
    q.execute(pool).await.unwrap();
}

#[tokio::test]
async fn fetch_stream_yields_every_row() {
    if dsn().is_none() {
        eprintln!("ORMX_TEST_PG_DSN not set — skipping");
        return;
    }
    let (pool, schema) = fresh_schema("yields_all").await;
    seed(&pool, 50).await;

    // The builder is borrowed by the stream — keep it alive in a let.
    let q = StreamUsers::all().order_by("id", ormx::asc());
    let mut stream = q.fetch_stream_as::<StreamUser>(&pool);

    let mut ids = Vec::new();
    while let Some(row) = stream.next().await {
        let u = row.unwrap();
        ids.push(u.id);
    }
    assert_eq!(ids.len(), 50);
    for i in 1..ids.len() {
        assert!(ids[i] > ids[i - 1], "not sorted at {i}");
    }

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn fetch_stream_supports_early_break() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("early_break").await;
    seed(&pool, 30).await;

    // Read first 5, drop the stream. The underlying connection is
    // returned to the pool when the stream is dropped.
    let mut n = 0;
    {
        let q = StreamUsers::all();
        let mut stream = q.fetch_stream_as::<StreamUser>(&pool);
        while let Some(row) = stream.next().await {
            row.unwrap();
            n += 1;
            if n == 5 { break; }
        }
    }
    assert_eq!(n, 5);

    // Pool still usable — fetch_all on the same pool returns the full set.
    let all: Vec<StreamUser> = StreamUsers::all().fetch_all_as::<StreamUser>(&pool).await.unwrap();
    assert_eq!(all.len(), 30);

    drop_schema(&pool, &schema).await;
}
