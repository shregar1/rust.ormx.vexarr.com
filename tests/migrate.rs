//! End-to-end migration tests against a real Postgres.
//!
//! Skipped automatically if `ORMX_TEST_PG_DSN` is unset, so this won't
//! fight CI in environments without a DB. Set it locally to:
//!   ORMX_TEST_PG_DSN=postgres://shreyansh@localhost:5432/ormx_test
//!
//! Each test owns its own schema (`mig_test_<n>`) to avoid cross-talk.

#![cfg(all(feature = "migrations", feature = "postgres"))]

use std::fs;
use std::path::PathBuf;

use ormx::migrate;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn dsn() -> Option<String> {
    std::env::var("ORMX_TEST_PG_DSN").ok()
}

/// Make a fresh schema, return `(pool_scoped_to_it, schema_name)`.
///
/// The pool is configured with `after_connect` so EVERY connection
/// (not just the one that ran `SET search_path`) lands in our isolated
/// schema. Without this hook, sqlx may borrow a different connection
/// for the next query and see the default search_path → "_ormx_migrations
/// does not exist".
async fn fresh_schema(name: &str) -> (PgPool, String) {
    let dsn = dsn().expect("ORMX_TEST_PG_DSN must be set");
    // Pid keeps schemas unique across concurrent test runs.
    let schema = format!("mig_test_{name}_{pid}", pid = std::process::id());

    // Set up the schema with an admin pool first (default search_path).
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&dsn)
        .await
        .unwrap();
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    // Test pool: pin search_path on every connection.
    let schema_for_hook = schema.clone();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _| {
            let s = schema_for_hook.clone();
            Box::pin(async move {
                sqlx::query(&format!("SET search_path TO {s}"))
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect(&dsn)
        .await
        .expect("connect");
    (pool, schema)
}

async fn drop_schema(pool: &PgPool, schema: &str) {
    let _ = sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(pool)
        .await;
}

/// Write a `<version>__<name>.up.sql` (+ optional down) pair under `dir`.
fn write_migration(dir: &PathBuf, version: &str, name: &str, up: &str, down: Option<&str>) {
    fs::write(dir.join(format!("{version}__{name}.up.sql")), up).unwrap();
    if let Some(d) = down {
        fs::write(dir.join(format!("{version}__{name}.down.sql")), d).unwrap();
    }
}

fn tmpdir(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("ormx_mig_{label}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

// ---------------------------------------------------------------------------

#[tokio::test]
async fn load_dir_sorts_by_version_and_pairs_up_down() {
    if dsn().is_none() {
        eprintln!("ORMX_TEST_PG_DSN not set — skipping");
        return;
    }
    let dir = tmpdir("load_sort");
    // Insert out of order on purpose
    write_migration(&dir, "0002", "users", "CREATE TABLE users (id INT)", Some("DROP TABLE users"));
    write_migration(&dir, "0001", "init", "SELECT 1", None);
    write_migration(&dir, "0003", "orders", "CREATE TABLE orders (id INT)", None);
    // A stray file we should ignore
    fs::write(dir.join("README.md"), "ignored").unwrap();

    let m = migrate::load_dir(&dir).unwrap();
    let versions: Vec<&str> = m.iter().map(|m| m.version.as_str()).collect();
    assert_eq!(versions, vec!["0001", "0002", "0003"]);
    assert_eq!(m[0].down_sql, None);
    assert!(m[1].down_sql.as_deref().unwrap().contains("DROP TABLE"));
}

#[tokio::test]
async fn up_then_status_records_each_migration() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("up_status").await;

    let dir = tmpdir("up_status");
    write_migration(&dir, "0001", "init",
        "CREATE TABLE widget (id SERIAL PRIMARY KEY, name TEXT)",
        Some("DROP TABLE widget"));
    write_migration(&dir, "0002", "add_count",
        "ALTER TABLE widget ADD COLUMN count INT NOT NULL DEFAULT 0",
        Some("ALTER TABLE widget DROP COLUMN count"));
    let migs = migrate::load_dir(&dir).unwrap();

    let applied = migrate::up(&pool, &migs, None).await.unwrap();
    assert_eq!(applied, vec!["0001".to_string(), "0002".to_string()]);

    // Status now shows both as applied, nothing pending, no drift.
    let plan = migrate::status(&pool, &migs).await.unwrap();
    assert_eq!(plan.applied.len(), 2);
    assert!(plan.pending.is_empty());
    assert!(plan.drift.is_empty());

    // And `widget.count` actually exists in the DB.
    let (cnt,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM widget")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(cnt, 0);

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn up_is_idempotent_when_run_twice() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("idempotent").await;

    let dir = tmpdir("idempotent");
    write_migration(&dir, "0001", "init",
        "CREATE TABLE thing (id INT)",
        Some("DROP TABLE thing"));
    let migs = migrate::load_dir(&dir).unwrap();

    let a = migrate::up(&pool, &migs, None).await.unwrap();
    let b = migrate::up(&pool, &migs, None).await.unwrap();
    assert_eq!(a, vec!["0001".to_string()]);
    assert!(b.is_empty(), "second run should be a no-op");

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn checksum_drift_is_surfaced_and_blocks_up() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("drift").await;

    let dir = tmpdir("drift");
    write_migration(&dir, "0001", "init",
        "CREATE TABLE drifted (id INT)",
        Some("DROP TABLE drifted"));
    let migs = migrate::load_dir(&dir).unwrap();
    migrate::up(&pool, &migs, None).await.unwrap();

    // User edits the migration after applying it.
    write_migration(&dir, "0001", "init",
        "CREATE TABLE drifted (id INT, edited BOOL)", // different now
        Some("DROP TABLE drifted"));
    let migs_edited = migrate::load_dir(&dir).unwrap();

    let plan = migrate::status(&pool, &migs_edited).await.unwrap();
    assert_eq!(plan.drift.len(), 1);
    assert_eq!(plan.drift[0].version, "0001");
    assert_ne!(plan.drift[0].recorded_checksum, plan.drift[0].disk_checksum);

    // Add a second migration on top — up() should refuse to proceed
    // until the drift is resolved.
    write_migration(&dir, "0002", "add_y", "ALTER TABLE drifted ADD COLUMN y INT", None);
    let migs_more = migrate::load_dir(&dir).unwrap();
    let err = migrate::up(&pool, &migs_more, None).await;
    assert!(err.is_err(), "expected drift to block up()");
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("drift"), "error should mention drift, got: {msg}");

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn down_rolls_back_one_step_by_default() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("down_one").await;

    let dir = tmpdir("down_one");
    write_migration(&dir, "0001", "init",
        "CREATE TABLE a (id INT)",
        Some("DROP TABLE a"));
    write_migration(&dir, "0002", "two",
        "CREATE TABLE b (id INT)",
        Some("DROP TABLE b"));
    let migs = migrate::load_dir(&dir).unwrap();

    migrate::up(&pool, &migs, None).await.unwrap();

    // Default: roll back exactly one (the newest).
    let rolled = migrate::down(&pool, &migs, None).await.unwrap();
    assert_eq!(rolled, vec!["0002".to_string()]);

    // `b` is gone, `a` still here.
    let exists_b: (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'b' AND table_schema = current_schema())")
        .fetch_one(&pool).await.unwrap();
    let exists_a: (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'a' AND table_schema = current_schema())")
        .fetch_one(&pool).await.unwrap();
    assert!(!exists_b.0, "b should be dropped");
    assert!(exists_a.0, "a should still exist");

    let plan = migrate::status(&pool, &migs).await.unwrap();
    assert_eq!(plan.applied.len(), 1);
    assert_eq!(plan.pending.len(), 1);

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn failed_migration_rolls_back_atomically() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("atomic").await;

    let dir = tmpdir("atomic");
    write_migration(&dir, "0001", "good",
        "CREATE TABLE good (id INT)",
        Some("DROP TABLE good"));
    // 0002 is intentionally broken — the second statement references
    // a column that doesn't exist. Postgres should reject the whole
    // transaction so the first statement is also rolled back, and
    // `_ormx_migrations` should not show 0002 as applied.
    write_migration(&dir, "0002", "broken",
        "CREATE TABLE broken (id INT);\nINSERT INTO broken (nope) VALUES (1);",
        None);
    let migs = migrate::load_dir(&dir).unwrap();

    let err = migrate::up(&pool, &migs, None).await;
    assert!(err.is_err());

    // First migration should have committed before the second one ran.
    let plan = migrate::status(&pool, &migs).await.unwrap();
    let applied_versions: Vec<&str> = plan.applied.iter().map(|a| a.version.as_str()).collect();
    assert_eq!(applied_versions, vec!["0001"]);
    // And the half-built table from 0002 should not exist.
    let exists_broken: (bool,) = sqlx::query_as("SELECT EXISTS (SELECT 1 FROM information_schema.tables WHERE table_name = 'broken' AND table_schema = current_schema())")
        .fetch_one(&pool).await.unwrap();
    assert!(!exists_broken.0, "broken table should have been rolled back");

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn down_refuses_when_down_sql_missing() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("no_down").await;

    let dir = tmpdir("no_down");
    write_migration(&dir, "0001", "one_way",
        "CREATE TABLE one_way (id INT)",
        None); // no down
    let migs = migrate::load_dir(&dir).unwrap();
    migrate::up(&pool, &migs, None).await.unwrap();

    let err = migrate::down(&pool, &migs, None).await;
    assert!(err.is_err());
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("no .down.sql"));

    drop_schema(&pool, &schema).await;
}

#[tokio::test]
async fn up_to_target_stops_at_that_version() {
    if dsn().is_none() { return; }
    let (pool, schema) = fresh_schema("up_target").await;

    let dir = tmpdir("up_target");
    write_migration(&dir, "0001", "a", "CREATE TABLE q (id INT)", Some("DROP TABLE q"));
    write_migration(&dir, "0002", "b", "CREATE TABLE r (id INT)", Some("DROP TABLE r"));
    write_migration(&dir, "0003", "c", "CREATE TABLE s (id INT)", Some("DROP TABLE s"));
    let migs = migrate::load_dir(&dir).unwrap();

    let applied = migrate::up(&pool, &migs, Some("0002")).await.unwrap();
    assert_eq!(applied, vec!["0001".to_string(), "0002".to_string()]);

    let plan = migrate::status(&pool, &migs).await.unwrap();
    assert_eq!(plan.pending.len(), 1);
    assert_eq!(plan.pending[0].version, "0003");

    drop_schema(&pool, &schema).await;
}
