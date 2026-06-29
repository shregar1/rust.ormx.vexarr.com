//! End-to-end against in-memory sqlite. Proves both execution paths work:
//! the generic Executor (JSON) and the native FromRow (zero-copy) path.

use futures_util::TryStreamExt;
use ormx::{Execute, LiteExecute, Model, Value, desc, eq, gt};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Deserialize, Serialize, PartialEq, sqlx::FromRow)]
struct User {
    id: i64,
    email: String,
    age: i64,
}

struct Users;
impl Model for Users {
    fn table_name() -> &'static str {
        "users"
    }
}

async fn setup() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL, age INTEGER NOT NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn insert_then_find_then_update_then_delete() {
    let pool = setup().await;

    // INSERT
    let inserted: User = Users::insert([
        ("id", Value::from(1)),
        ("email", Value::from("a@b.com")),
        ("age", Value::from(30)),
    ])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(inserted, User { id: 1, email: "a@b.com".into(), age: 30 });

    // FIND
    let found: User = Users::find(1).fetch_one(&pool).await.unwrap();
    assert_eq!(found.email, "a@b.com");

    // COUNT — comes back as { "COUNT(*)": 1 }; deserialise into a tagged struct.
    #[derive(Deserialize)]
    struct Count { #[serde(rename = "COUNT(*)")] n: i64 }
    let c: Count = Users::count().fetch_one(&pool).await.unwrap();
    assert_eq!(c.n, 1);

    // UPDATE
    let affected = Users::update(1, [("age", Value::from(31))])
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(affected, 1);
    let after: User = Users::find(1).fetch_one(&pool).await.unwrap();
    assert_eq!(after.age, 31);

    // DELETE
    let affected = Users::delete(1).execute(&pool).await.unwrap();
    assert_eq!(affected, 1);
    let none: Option<User> = Users::find(1).fetch_optional(&pool).await.unwrap();
    assert!(none.is_none());
}

#[tokio::test]
async fn filter_order_limit() {
    let pool = setup().await;
    Users::insert_many(vec![
        vec![("id", Value::from(1)), ("email", Value::from("a@b.com")), ("age", Value::from(20))],
        vec![("id", Value::from(2)), ("email", Value::from("c@d.com")), ("age", Value::from(40))],
        vec![("id", Value::from(3)), ("email", Value::from("e@f.com")), ("age", Value::from(60))],
    ])
    .execute(&pool)
    .await
    .unwrap();

    let adults: Vec<User> = Users::where_("age", gt(25))
        .order_by("age", desc())
        .limit(2)
        .fetch_all(&pool)
        .await
        .unwrap();

    assert_eq!(adults.len(), 2);
    assert_eq!(adults[0].age, 60);
    assert_eq!(adults[1].age, 40);
}

#[tokio::test]
async fn upsert_do_nothing() {
    let pool = setup().await;
    sqlx::query("CREATE UNIQUE INDEX users_email ON users(email)")
        .execute(&pool)
        .await
        .unwrap();

    Users::insert([
        ("id", Value::from(1)),
        ("email", Value::from("a@b.com")),
        ("age", Value::from(20)),
    ])
    .execute(&pool)
    .await
    .unwrap();

    // Same email — should be a no-op.
    let affected = Users::insert([
        ("id", Value::from(2)),
        ("email", Value::from("a@b.com")),
        ("age", Value::from(99)),
    ])
    .on_conflict("email")
    .do_nothing()
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(affected, 0);

    let found: User = Users::where_("email", eq("a@b.com"))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(found.age, 20); // original, unchanged
}

#[tokio::test]
async fn raw_escape_hatch() {
    let pool = setup().await;
    Users::insert([
        ("id", Value::from(1)),
        ("email", Value::from("a@b.com")),
        ("age", Value::from(30)),
    ])
    .execute(&pool)
    .await
    .unwrap();

    let rows: Vec<User> = ormx::raw("SELECT * FROM users WHERE age = ?", [Value::from(30)])
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].email, "a@b.com");
}

#[tokio::test]
async fn transaction_commit_through_builders() {
    let pool = setup().await;

    let mut tx = pool.begin().await.unwrap();

    Users::insert([
        ("id", Value::from(1)),
        ("email", Value::from("a@b.com")),
        ("age", Value::from(30)),
    ])
    .execute_in(&mut tx)
    .await
    .unwrap();

    let inside: User = Users::find(1).fetch_one_as_in::<User>(&mut tx).await.unwrap();
    assert_eq!(inside.email, "a@b.com");

    tx.commit().await.unwrap();

    // Visible after commit.
    let after: User = Users::find(1).fetch_one_as::<User>(&pool).await.unwrap();
    assert_eq!(after.email, "a@b.com");
}

#[tokio::test]
async fn transaction_rollback_through_builders() {
    let pool = setup().await;

    let mut tx = pool.begin().await.unwrap();

    Users::insert([
        ("id", Value::from(42)),
        ("email", Value::from("ghost@example.com")),
        ("age", Value::from(99)),
    ])
    .execute_in(&mut tx)
    .await
    .unwrap();

    // Visible inside the tx.
    let inside: Option<User> = Users::find(42)
        .fetch_optional_as_in::<User>(&mut tx)
        .await
        .unwrap();
    assert!(inside.is_some());

    tx.rollback().await.unwrap();

    // Gone after rollback.
    let outside: Option<User> = Users::find(42)
        .fetch_optional_as::<User>(&pool)
        .await
        .unwrap();
    assert!(outside.is_none());
}

#[tokio::test]
async fn transaction_mixed_with_pool_after_commit() {
    let pool = setup().await;

    // Seed via pool.
    Users::insert([
        ("id", Value::from(1)),
        ("email", Value::from("a@b.com")),
        ("age", Value::from(20)),
    ])
    .execute_on(&pool)
    .await
    .unwrap();

    // Bump age inside a tx using both fetch and update.
    let mut tx = pool.begin().await.unwrap();
    let before: User = Users::find(1).fetch_one_as_in::<User>(&mut tx).await.unwrap();
    assert_eq!(before.age, 20);

    Users::update(1, [("age", Value::from(21))])
        .execute_in(&mut tx)
        .await
        .unwrap();

    let all_inside: Vec<User> = Users::all().fetch_all_as_in::<User>(&mut tx).await.unwrap();
    assert_eq!(all_inside.len(), 1);
    assert_eq!(all_inside[0].age, 21);

    tx.commit().await.unwrap();

    let after: User = Users::find(1).fetch_one_as::<User>(&pool).await.unwrap();
    assert_eq!(after.age, 21);
}

#[tokio::test]
async fn streams_all_rows_without_buffering() {
    let pool = setup().await;
    // Seed 1000 rows. Streaming should walk them one-by-one.
    let rows: Vec<Vec<(&str, Value)>> = (1..=1000)
        .map(|i| {
            vec![
                ("id", Value::from(i)),
                ("email", Value::from(format!("u{}@example.com", i))),
                ("age", Value::from(20 + (i % 50))),
            ]
        })
        .collect();
    Users::insert_many(rows).execute_on(&pool).await.unwrap();

    let q = Users::all();
    let mut stream = q.fetch_stream_as::<User>(&pool);
    let mut count = 0i64;
    let mut age_sum = 0i64;
    while let Some(user) = stream.try_next().await.unwrap() {
        count += 1;
        age_sum += user.age;
    }
    assert_eq!(count, 1000);
    assert!(age_sum > 0);
}

#[tokio::test]
async fn streams_with_filter_and_order() {
    let pool = setup().await;
    Users::insert_many(vec![
        vec![("id", Value::from(1)), ("email", Value::from("a@b.com")), ("age", Value::from(20))],
        vec![("id", Value::from(2)), ("email", Value::from("c@d.com")), ("age", Value::from(40))],
        vec![("id", Value::from(3)), ("email", Value::from("e@f.com")), ("age", Value::from(60))],
    ])
    .execute_on(&pool)
    .await
    .unwrap();

    let q = Users::where_("age", gt(25)).order_by("age", desc());
    let mut stream = q.fetch_stream_as::<User>(&pool);

    let first = stream.try_next().await.unwrap().unwrap();
    let second = stream.try_next().await.unwrap().unwrap();
    assert!(stream.try_next().await.unwrap().is_none());

    assert_eq!(first.age, 60);
    assert_eq!(second.age, 40);
}

#[tokio::test]
async fn streams_inside_a_transaction() {
    let pool = setup().await;
    Users::insert_many(vec![
        vec![("id", Value::from(1)), ("email", Value::from("a@b.com")), ("age", Value::from(20))],
        vec![("id", Value::from(2)), ("email", Value::from("c@d.com")), ("age", Value::from(40))],
    ])
    .execute_on(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();

    // Apply an uncommitted change…
    Users::update(1, [("age", Value::from(999))])
        .execute_in(&mut tx)
        .await
        .unwrap();

    // …then stream from inside the same tx — should see the change.
    {
        let q = Users::all();
        let mut stream = q.fetch_stream_as_in::<User>(&mut tx);
        let mut saw_999 = false;
        while let Some(user) = stream.try_next().await.unwrap() {
            if user.id == 1 && user.age == 999 {
                saw_999 = true;
            }
        }
        assert!(saw_999);
    }

    tx.rollback().await.unwrap();

    // After rollback, the pool view is back to original.
    let original: User = Users::find(1).fetch_one_as::<User>(&pool).await.unwrap();
    assert_eq!(original.age, 20);
}

#[tokio::test]
async fn native_fast_path_matches_generic_path() {
    let pool = setup().await;
    Users::insert_many(vec![
        vec![("id", Value::from(1)), ("email", Value::from("a@b.com")), ("age", Value::from(20))],
        vec![("id", Value::from(2)), ("email", Value::from("c@d.com")), ("age", Value::from(40))],
    ])
    .execute_on(&pool)
    .await
    .unwrap();

    // Native FromRow path — no JSON round-trip.
    let native: Vec<User> = Users::where_("age", gt(0))
        .order_by("id", desc())
        .fetch_all_as::<User>(&pool)
        .await
        .unwrap();

    // Generic JSON path.
    let generic: Vec<User> = Users::where_("age", gt(0))
        .order_by("id", desc())
        .fetch_all(&pool)
        .await
        .unwrap();

    assert_eq!(native, generic);
    assert_eq!(native.len(), 2);

    // Single-row variants.
    let one_native: User = Users::find(1).fetch_one_as::<User>(&pool).await.unwrap();
    let none_native: Option<User> = Users::find(999)
        .fetch_optional_as::<User>(&pool)
        .await
        .unwrap();
    assert_eq!(one_native.id, 1);
    assert!(none_native.is_none());
}
