//! Decomposed perf harness — separates the cost dimensions.
//!
//! Inspired by qx-storage's `append` / `append_sync` split: the value of
//! this bench is in the *delta* between the two groups, not the absolute
//! number. Subtract the no-tx group from the tx group and you get the
//! cost of a BEGIN/COMMIT round-trip on this driver.
//!
//! Groups (all SQLite in-memory — minimal wire cost, mostly CPU):
//!
//!   select_one         { no_tx, tx }
//!     no_tx → wire + decode for a single fetched row
//!     tx    → wire + decode + BEGIN/COMMIT
//!     Δ     → durability bookkeeping per call
//!
//!   select_many/<rows>
//!     fetch_all of N rows, no tx; reports ns/call and rows/µs
//!
//!   insert_one         { no_tx, tx }
//!     no_tx → wire + bind for a single INSERT
//!     tx    → wire + bind + BEGIN/COMMIT
//!     Δ     → durability bookkeeping per call
//!
//!   insert_batch/<rows>
//!     N-row multi-VALUES INSERT, no tx; throughput in rows/sec
//!
//! On SQLite-in-memory the tx delta is small (no fsync). Against real
//! Postgres the delta is the durability round-trip — typically 100–500µs
//! and dominated by network + WAL fsync. Reading the delta as its own
//! number tells you whether your hot path should batch or auto-commit.
//!
//! Run with: `cargo bench --bench decomposed`.

use std::cell::Cell;
use std::time::Instant;

use ormx::{LiteExecute, Model, Value};
use serde::Deserialize;
use sqlx::SqlitePool;

#[derive(Debug, Deserialize, sqlx::FromRow)]
struct User {
    #[allow(dead_code)]
    id: i64,
    #[allow(dead_code)]
    email: String,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    age: i64,
}

struct Users;
impl Model for Users {
    fn table_name() -> &'static str { "users" }
}

async fn setup() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE users (
            id    INTEGER PRIMARY KEY,
            email TEXT NOT NULL,
            name  TEXT NOT NULL,
            age   INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    for i in 1..=100i64 {
        sqlx::query("INSERT INTO users VALUES (?, ?, ?, ?)")
            .bind(i)
            .bind(format!("u{}@x.com", i))
            .bind(format!("U{}", i))
            .bind(20 + (i % 50))
            .execute(&pool)
            .await
            .unwrap();
    }
    pool
}

async fn cleanup(pool: &SqlitePool) {
    sqlx::query("DELETE FROM users WHERE id > 100")
        .execute(pool)
        .await
        .unwrap();
}

async fn best_of<F, Fut>(samples: usize, mut f: F) -> u128
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = u128>,
{
    let mut best = u128::MAX;
    for _ in 0..samples {
        let t = f().await;
        if t < best {
            best = t;
        }
    }
    best
}

fn ns_per_op(total_ns: u128, n: usize) -> f64 {
    total_ns as f64 / n as f64
}

fn rows_per_sec(elapsed_ns: u128, n_rows: usize) -> f64 {
    (n_rows as f64) / (elapsed_ns as f64 / 1e9)
}

const N_SELECT: usize = 5_000;
const N_INSERT: usize = 2_000;
const SAMPLES: usize = 3;

// ---- select_one ---------------------------------------------------------

async fn select_one_no_tx(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N_SELECT {
        let _: User = Users::find(1).fetch_one_as::<User>(pool).await.unwrap();
    }
    start.elapsed().as_nanos()
}

async fn select_one_tx(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N_SELECT {
        let mut tx = pool.begin().await.unwrap();
        let _: User = Users::find(1).fetch_one_as_in::<User>(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    start.elapsed().as_nanos()
}

// ---- select_many --------------------------------------------------------

async fn select_many_no_tx(pool: &SqlitePool, rows: usize) -> u128 {
    let iters = (N_SELECT / rows.max(1)).max(50);
    let start = Instant::now();
    for _ in 0..iters {
        let _: Vec<User> = Users::all()
            .limit(rows as u64)
            .fetch_all_as::<User>(pool)
            .await
            .unwrap();
    }
    start.elapsed().as_nanos() / (iters as u128)
}

// ---- insert_one ---------------------------------------------------------

async fn insert_one_no_tx(pool: &SqlitePool, base_id: i64) -> u128 {
    let start = Instant::now();
    for i in 0..N_INSERT as i64 {
        let id = base_id + i;
        Users::insert(vec![
            ("id", Value::from(id)),
            ("email", Value::from(format!("z{}@x.com", id).as_str())),
            ("name", Value::from("Z")),
            ("age", Value::from(30i64)),
        ])
        .execute_on(pool)
        .await
        .unwrap();
    }
    start.elapsed().as_nanos()
}

async fn insert_one_tx(pool: &SqlitePool, base_id: i64) -> u128 {
    let start = Instant::now();
    for i in 0..N_INSERT as i64 {
        let id = base_id + i;
        let mut tx = pool.begin().await.unwrap();
        Users::insert(vec![
            ("id", Value::from(id)),
            ("email", Value::from(format!("z{}@x.com", id).as_str())),
            ("name", Value::from("Z")),
            ("age", Value::from(30i64)),
        ])
        .execute_in(&mut tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
    start.elapsed().as_nanos()
}

// ---- insert_batch -------------------------------------------------------

// Chunked at 1000 rows per multi-VALUES statement. SQLite caps bound
// params at 32766 (4 cols × 8200 rows ≈ ceiling); Postgres caps at 65535.
// 1000 is the standard chunk we use across all ormx SDKs.
const CHUNK: usize = 1000;

async fn insert_batch(pool: &SqlitePool, base_id: i64, n_rows: usize) -> u128 {
    let mut chunks: Vec<Vec<Vec<(&str, Value)>>> = Vec::new();
    for chunk_start in (0..n_rows).step_by(CHUNK) {
        let chunk_end = (chunk_start + CHUNK).min(n_rows);
        let rows: Vec<Vec<(&str, Value)>> = (chunk_start..chunk_end)
            .map(|i| {
                let id = base_id + i as i64;
                vec![
                    ("id", Value::from(id)),
                    ("email", Value::from(format!("z{}@x.com", id))),
                    ("name", Value::from("Z")),
                    ("age", Value::from(30i64)),
                ]
            })
            .collect();
        chunks.push(rows);
    }
    let start = Instant::now();
    for rows in chunks {
        Users::insert_many(rows).execute_on(pool).await.unwrap();
    }
    start.elapsed().as_nanos()
}

// ---- runner -------------------------------------------------------------

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let pool = setup().await;

    // warmup — prime prepared-statement cache + arena allocations
    for _ in 0..500 {
        let _: User = Users::find(1).fetch_one_as::<User>(&pool).await.unwrap();
    }

    println!("\normx-rust-client / decomposed bench (SQLite in-mem)");
    println!("====================================================");
    println!("Best of {} samples. Δ = tx − no_tx (BEGIN/COMMIT cost).", SAMPLES);

    // Bind a reference up front so each FnMut closure below captures the
    // reference (Copy) instead of trying to move `pool` itself.
    let pool_ref = &pool;

    // ---- select_one ----
    let notx = best_of(SAMPLES, || select_one_no_tx(pool_ref)).await;
    let tx = best_of(SAMPLES, || select_one_tx(pool_ref)).await;
    println!("\nselect_one  ({}× iters per sample)", N_SELECT);
    println!("  no_tx  {:>9.0} ns/op", ns_per_op(notx, N_SELECT));
    println!("  tx     {:>9.0} ns/op", ns_per_op(tx, N_SELECT));
    println!(
        "  Δ      {:>9.0} ns/op  ({:.2}× of no_tx)",
        ns_per_op(tx.saturating_sub(notx), N_SELECT),
        tx as f64 / notx as f64
    );

    // ---- select_many ----
    println!("\nselect_many  (no tx, ns/call · rows/µs)");
    for &rows in &[10usize, 100] {
        let t = best_of(SAMPLES, || select_many_no_tx(pool_ref, rows)).await;
        println!(
            "  {:>3} rows  {:>9.0} ns/call  ({:.1} rows/µs)",
            rows,
            t as f64,
            rows as f64 * 1000.0 / t as f64
        );
    }

    // ---- insert_one ----
    // best_of needs distinct id ranges per sample to avoid PK collisions;
    // Cell lets the FnMut closure advance the cursor between samples.
    let id_base = Cell::new(1_000_000i64);
    let id_ref = &id_base;
    let notx = best_of(SAMPLES, || {
        let b = id_ref.get();
        id_ref.set(b + N_INSERT as i64);
        async move {
            let t = insert_one_no_tx(pool_ref, b).await;
            cleanup(pool_ref).await;
            t
        }
    })
    .await;
    let tx = best_of(SAMPLES, || {
        let b = id_ref.get();
        id_ref.set(b + N_INSERT as i64);
        async move {
            let t = insert_one_tx(pool_ref, b).await;
            cleanup(pool_ref).await;
            t
        }
    })
    .await;
    println!("\ninsert_one  ({}× iters per sample)", N_INSERT);
    println!("  no_tx  {:>9.0} ns/op", ns_per_op(notx, N_INSERT));
    println!("  tx     {:>9.0} ns/op", ns_per_op(tx, N_INSERT));
    println!(
        "  Δ      {:>9.0} ns/op  ({:.2}× of no_tx)",
        ns_per_op(tx.saturating_sub(notx), N_INSERT),
        tx as f64 / notx as f64
    );

    // ---- insert_batch ----
    println!("\ninsert_batch  (multi-VALUES, no tx)");
    for &n in &[100usize, 1_000, 10_000] {
        let t = best_of(SAMPLES, || {
            let b = id_ref.get();
            id_ref.set(b + n as i64);
            async move {
                let elapsed = insert_batch(pool_ref, b, n).await;
                cleanup(pool_ref).await;
                elapsed
            }
        })
        .await;
        println!(
            "  {:>5} rows  {:>9.2} ms  ({:>8.0} rows/sec)",
            n,
            t as f64 / 1e6,
            rows_per_sec(t, n)
        );
    }
    println!();
}
