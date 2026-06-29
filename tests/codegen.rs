//! End-to-end codegen tests against a real Postgres.
//!
//! Skipped when `ORMX_TEST_PG_DSN` is unset, so CI without a DB stays
//! green. Each test owns its own schema (`gen_test_<n>_<pid>`).

#![cfg(all(feature = "codegen", feature = "postgres"))]

use ormx::codegen;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn dsn() -> Option<String> {
    std::env::var("ORMX_TEST_PG_DSN").ok()
}

async fn fresh_schema(name: &str) -> (PgPool, String) {
    let dsn = dsn().expect("ORMX_TEST_PG_DSN must be set");
    let schema = format!("gen_test_{name}_{pid}", pid = std::process::id());

    let admin = PgPoolOptions::new().max_connections(1).connect(&dsn).await.unwrap();
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin).await.unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin).await.unwrap();
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
    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(pool).await;
}

#[tokio::test]
async fn introspect_basic_table_with_pk_and_nullable() {
    if dsn().is_none() {
        eprintln!("ORMX_TEST_PG_DSN not set — skipping");
        return;
    }
    let (pool, schema) = fresh_schema("basic").await;

    sqlx::query(
        "CREATE TABLE users (
            id        BIGSERIAL PRIMARY KEY,
            email     TEXT NOT NULL,
            nickname  TEXT,
            age       INT NOT NULL,
            active    BOOLEAN NOT NULL DEFAULT TRUE
        )",
    )
    .execute(&pool).await.unwrap();

    let tables = codegen::introspect(&pool, &schema, None).await.unwrap();
    assert_eq!(tables.len(), 1);
    let t = &tables[0];
    assert_eq!(t.name, "users");

    // Columns come back in ordinal order.
    let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["id", "email", "nickname", "age", "active"]);

    // PK = id only.
    let pks: Vec<&str> = t.columns.iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str()).collect();
    assert_eq!(pks, vec!["id"]);

    // Nullable = nickname only.
    let nulls: Vec<&str> = t.columns.iter()
        .filter(|c| c.nullable)
        .map(|c| c.name.as_str()).collect();
    assert_eq!(nulls, vec!["nickname"]);

    // Rendered code is sensible.
    let rust = codegen::render_rust(t);
    assert!(rust.contains("pub struct Users {"), "got: {rust}");
    assert!(rust.contains("pub id: i64,"));
    assert!(rust.contains("pub email: String,"));
    assert!(rust.contains("pub nickname: Option<String>,"));
    assert!(rust.contains("pub age: i32,"));
    assert!(rust.contains("pub active: bool,"));
    assert!(rust.contains("fn table_name() -> &'static str { \"users\" }"));

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn introspect_multiple_tables_sorted_by_name() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("multi").await;

    // On purpose, create them out of alphabetical order.
    sqlx::query("CREATE TABLE z_widgets (id INT PRIMARY KEY)")
        .execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE accounts (id INT PRIMARY KEY)")
        .execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE orders (id INT PRIMARY KEY, account_id INT NOT NULL)")
        .execute(&pool).await.unwrap();

    let tables = codegen::introspect(&pool, &schema, None).await.unwrap();
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["accounts", "orders", "z_widgets"]);

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn introspect_filter_by_table_name() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("filter").await;

    sqlx::query("CREATE TABLE a (id INT PRIMARY KEY)").execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE b (id INT PRIMARY KEY)").execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE c (id INT PRIMARY KEY)").execute(&pool).await.unwrap();

    let tables = codegen::introspect(&pool, &schema, Some(&["a", "c"])).await.unwrap();
    let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["a", "c"]);

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn introspect_handles_all_common_pg_types() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("types").await;

    sqlx::query(
        "CREATE TABLE every_type (
            c_int2   SMALLINT,
            c_int4   INTEGER NOT NULL,
            c_int8   BIGINT NOT NULL,
            c_real   REAL,
            c_dbl    DOUBLE PRECISION,
            c_bool   BOOLEAN NOT NULL,
            c_text   TEXT NOT NULL,
            c_varch  VARCHAR(64),
            c_uuid   UUID,
            c_jsonb  JSONB NOT NULL,
            c_ts     TIMESTAMP NOT NULL,
            c_tstz   TIMESTAMPTZ,
            c_date   DATE,
            c_bytea  BYTEA
        )",
    )
    .execute(&pool).await.unwrap();

    let tables = codegen::introspect(&pool, &schema, None).await.unwrap();
    let t = tables.iter().find(|t| t.name == "every_type").unwrap();
    let rust = codegen::render_rust(t);

    // Spot-check that each column rendered with the expected Rust type.
    assert!(rust.contains("pub c_int2: Option<i16>,"), "got: {rust}");
    assert!(rust.contains("pub c_int4: i32,"));
    assert!(rust.contains("pub c_int8: i64,"));
    assert!(rust.contains("pub c_real: Option<f32>,"));
    assert!(rust.contains("pub c_dbl: Option<f64>,"));
    assert!(rust.contains("pub c_bool: bool,"));
    assert!(rust.contains("pub c_text: String,"));
    assert!(rust.contains("pub c_varch: Option<String>,"));
    assert!(rust.contains("pub c_uuid: Option<uuid::Uuid>,"));
    assert!(rust.contains("pub c_jsonb: serde_json::Value,"));
    assert!(rust.contains("pub c_ts: chrono::NaiveDateTime,"));
    assert!(rust.contains("pub c_tstz: Option<chrono::DateTime<chrono::Utc>>,"));
    assert!(rust.contains("pub c_date: Option<chrono::NaiveDate>,"));
    assert!(rust.contains("pub c_bytea: Option<Vec<u8>>,"));

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn introspect_composite_primary_key() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("composite_pk").await;

    sqlx::query(
        "CREATE TABLE membership (
            org_id  INT NOT NULL,
            user_id INT NOT NULL,
            role    TEXT NOT NULL,
            PRIMARY KEY (org_id, user_id)
        )",
    )
    .execute(&pool).await.unwrap();

    let tables = codegen::introspect(&pool, &schema, None).await.unwrap();
    let t = tables.iter().find(|t| t.name == "membership").unwrap();
    let pks: Vec<&str> = t.columns.iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str()).collect();
    assert_eq!(pks, vec!["org_id", "user_id"]);

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn introspect_empty_schema_returns_empty() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("empty").await;
    let tables = codegen::introspect(&pool, &schema, None).await.unwrap();
    assert!(tables.is_empty());
    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn render_rust_all_round_trip() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("render_all").await;

    sqlx::query("CREATE TABLE alpha (id INT PRIMARY KEY, name TEXT NOT NULL)")
        .execute(&pool).await.unwrap();
    sqlx::query("CREATE TABLE beta (id INT PRIMARY KEY, alpha_id INT NOT NULL)")
        .execute(&pool).await.unwrap();

    let tables = codegen::introspect(&pool, &schema, None).await.unwrap();
    let combined = codegen::render_rust_all(&tables);
    assert!(combined.contains("Generated by `ormx::codegen`"));
    assert!(combined.contains("pub struct Alpha {"));
    assert!(combined.contains("pub struct Beta {"));
    // Each table only renders once.
    assert_eq!(combined.matches("pub struct Alpha").count(), 1);
    assert_eq!(combined.matches("pub struct Beta").count(), 1);

    drop_schema(&pool, &schema).await;
}
