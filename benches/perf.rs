//! Perf harness: compares the three execution paths on a hot loop against
//! in-memory SQLite. SQLite-in-memory removes network/disk so what we measure
//! is essentially CPU spent decoding rows.
//!
//! Paths under test:
//!   1. raw sqlx           — bare `sqlx::query_as::<_, User>` + bind
//!   2. ormx native        — `Users::find(1).fetch_one_as::<User>(&pool)`
//!   3. ormx JSON          — `Users::find(1).fetch_one::<User>(&pool)`  (goes via serde_json)
//!
//! Run with: `cargo bench` (release profile).
//! The numbers we care about: native should be within ~5% of raw sqlx;
//! the JSON path is the baseline we improved from.

use std::time::Instant;

use ormx::{Execute, LiteExecute, Model};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

// NB: the JSON path probes column types at runtime — SQLite doesn't have a
// native BOOLEAN, so a bool column would deserialize as integer and fail.
// Using i64 here keeps both paths honest for the comparison.
#[derive(Debug, Deserialize, Serialize, sqlx::FromRow)]
struct User {
    id: i64,
    email: String,
    name: String,
    age: i64,
    bio: Option<String>,
    score: f64,
    active: i64,
}

struct Users;
impl Model for Users {
    fn table_name() -> &'static str {
        "users"
    }
}

const N: usize = 5_000;

async fn setup() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE users (
            id     INTEGER PRIMARY KEY,
            email  TEXT NOT NULL,
            name   TEXT NOT NULL,
            age    INTEGER NOT NULL,
            bio    TEXT,
            score  REAL NOT NULL,
            active INTEGER NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Seed 100 rows so the SELECT * has work to do.
    for i in 1..=100i64 {
        sqlx::query("INSERT INTO users VALUES (?, ?, ?, ?, ?, ?, ?)")
            .bind(i)
            .bind(format!("user{}@example.com", i))
            .bind(format!("User {}", i))
            .bind(20 + (i % 50))
            .bind(if i % 2 == 0 { Some(format!("bio for user {}", i)) } else { None })
            .bind(0.5 * i as f64)
            .bind((i % 3 == 0) as i64)
            .execute(&pool)
            .await
            .unwrap();
    }
    pool
}

fn ns_per_op(elapsed_ns: u128, n: usize) -> f64 {
    elapsed_ns as f64 / n as f64
}

/// Run a closure `samples` times and return the best (lowest) elapsed time.
/// Best-of-N reduces OS-scheduler noise on a non-quiet machine.
async fn best_of<F, Fut>(samples: usize, mut f: F) -> u128
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = u128>,
{
    let mut best = u128::MAX;
    for _ in 0..samples {
        let t = f().await;
        if t < best { best = t; }
    }
    best
}

async fn bench_raw_sqlx_fetch_one(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N {
        let _: User = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ? LIMIT 1")
            .bind(1i64)
            .fetch_one(pool)
            .await
            .unwrap();
    }
    start.elapsed().as_nanos()
}

async fn bench_ormx_native_fetch_one(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N {
        let _: User = Users::find(1).fetch_one_as::<User>(pool).await.unwrap();
    }
    start.elapsed().as_nanos()
}

async fn bench_ormx_json_fetch_one(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N {
        let _: User = Users::find(1).fetch_one::<_, User>(pool).await.unwrap();
    }
    start.elapsed().as_nanos()
}

async fn bench_raw_sqlx_fetch_all(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N {
        let _: Vec<User> = sqlx::query_as::<_, User>("SELECT * FROM users")
            .fetch_all(pool)
            .await
            .unwrap();
    }
    start.elapsed().as_nanos()
}

async fn bench_ormx_native_fetch_all(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N {
        let _: Vec<User> = Users::all().fetch_all_as::<User>(pool).await.unwrap();
    }
    start.elapsed().as_nanos()
}

async fn bench_ormx_json_fetch_all(pool: &SqlitePool) -> u128 {
    let start = Instant::now();
    for _ in 0..N {
        let _: Vec<User> = Users::all().fetch_all::<_, User>(pool).await.unwrap();
    }
    start.elapsed().as_nanos()
}

fn report(label: &str, ns: u128, baseline_ns: u128) {
    let per_op = ns_per_op(ns, N);
    let ratio = ns as f64 / baseline_ns as f64;
    println!(
        "  {:<28} {:>8.0} ns/op   ({:>4.2}x baseline)",
        label, per_op, ratio
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let pool = setup().await;

    // Warm up — first query compiles the prepared statement & primes caches.
    for _ in 0..500 {
        let _: User = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let _: Vec<User> = sqlx::query_as::<_, User>("SELECT * FROM users")
            .fetch_all(&pool)
            .await
            .unwrap();
    }

    const SAMPLES: usize = 3;

    println!("\n=== fetch_one (1 row, 7 cols), N = {}, best of {} ===", N, SAMPLES);
    let raw = best_of(SAMPLES, || bench_raw_sqlx_fetch_one(&pool)).await;
    let nat = best_of(SAMPLES, || bench_ormx_native_fetch_one(&pool)).await;
    let jsn = best_of(SAMPLES, || bench_ormx_json_fetch_one(&pool)).await;
    report("raw sqlx",    raw, raw);
    report("ormx native", nat, raw);
    report("ormx json",   jsn, raw);

    println!("\n=== fetch_all (100 rows, 7 cols), N = {}, best of {} ===", N, SAMPLES);
    let raw = best_of(SAMPLES, || bench_raw_sqlx_fetch_all(&pool)).await;
    let nat = best_of(SAMPLES, || bench_ormx_native_fetch_all(&pool)).await;
    let jsn = best_of(SAMPLES, || bench_ormx_json_fetch_all(&pool)).await;
    report("raw sqlx",    raw, raw);
    report("ormx native", nat, raw);
    report("ormx json",   jsn, raw);
}
