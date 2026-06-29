//! Pure SQL-generation tests. No DB required.
//!
//! Every builder must implement `to_sql(dialect) -> (String, Vec<Value>)` so we
//! can lock in the contract without touching a database.

use ormx::{
    Dialect, Model, ToSql, Value,
    and, asc, between, desc, eq, gt, gte, ilike, in_, is_null, like, lte, ne, not_in,
    not_null, or, pred,
};

struct User;
impl Model for User {
    fn table_name() -> &'static str {
        "users"
    }
}

fn pg<B: ormx::ToSql>(b: B) -> (String, Vec<Value>) {
    b.to_sql(Dialect::Postgres)
}

// ---------------------------------------------------------------------------
// Model basics
// ---------------------------------------------------------------------------

#[test]
fn all_emits_plain_select() {
    let (sql, params) = pg(User::all());
    assert_eq!(sql, "SELECT * FROM users");
    assert!(params.is_empty());
}

#[test]
fn find_filters_by_id_and_limits_one() {
    let (sql, params) = pg(User::find(1));
    assert_eq!(sql, "SELECT * FROM users WHERE id = $1 LIMIT 1");
    assert_eq!(params, vec![Value::from(1)]);
}

#[test]
fn first_limits_one() {
    let (sql, params) = pg(User::first());
    assert_eq!(sql, "SELECT * FROM users LIMIT 1");
    assert!(params.is_empty());
}

#[test]
fn count_emits_count_star() {
    let (sql, params) = pg(User::count());
    assert_eq!(sql, "SELECT COUNT(*) FROM users");
    assert!(params.is_empty());
}

#[test]
fn exists_emits_exists_subquery() {
    let (sql, params) = pg(User::exists());
    assert_eq!(sql, "SELECT EXISTS(SELECT 1 FROM users)");
    assert!(params.is_empty());
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

#[test]
fn where_eq() {
    let (sql, params) = pg(User::where_("email", eq("a@b.com")));
    assert_eq!(sql, "SELECT * FROM users WHERE email = $1");
    assert_eq!(params, vec![Value::from("a@b.com")]);
}

#[test]
fn where_gt() {
    let (sql, params) = pg(User::where_("age", gt(18)));
    assert_eq!(sql, "SELECT * FROM users WHERE age > $1");
    assert_eq!(params, vec![Value::from(18)]);
}

#[test]
fn where_chains_with_and() {
    let (sql, params) = pg(User::where_("age", gt(18)).where_("name", eq("Ada")));
    assert_eq!(sql, "SELECT * FROM users WHERE age > $1 AND name = $2");
    assert_eq!(params, vec![Value::from(18), Value::from("Ada")]);
}

#[test]
fn where_like_and_ilike() {
    let (sql, _) = pg(User::where_("name", like("%shrey%")));
    assert_eq!(sql, "SELECT * FROM users WHERE name LIKE $1");

    let (sql, _) = pg(User::where_("name", ilike("ada%")));
    assert_eq!(sql, "SELECT * FROM users WHERE name ILIKE $1");
}

#[test]
fn where_ne_gte_lte() {
    let (sql, _) = pg(User::where_("a", ne(1)));
    assert_eq!(sql, "SELECT * FROM users WHERE a <> $1");

    let (sql, _) = pg(User::where_("a", gte(1)));
    assert_eq!(sql, "SELECT * FROM users WHERE a >= $1");

    let (sql, _) = pg(User::where_("a", lte(1)));
    assert_eq!(sql, "SELECT * FROM users WHERE a <= $1");
}

#[test]
fn where_between() {
    let (sql, params) = pg(User::where_("age", between(18, 65)));
    assert_eq!(sql, "SELECT * FROM users WHERE age BETWEEN $1 AND $2");
    assert_eq!(params, vec![Value::from(18), Value::from(65)]);
}

#[test]
fn where_in_and_not_in() {
    let (sql, params) = pg(User::where_("id", in_(vec![1, 2, 3])));
    assert_eq!(sql, "SELECT * FROM users WHERE id IN ($1, $2, $3)");
    assert_eq!(
        params,
        vec![Value::from(1), Value::from(2), Value::from(3)]
    );

    let (sql, _) = pg(User::where_("id", not_in(vec![1, 2])));
    assert_eq!(sql, "SELECT * FROM users WHERE id NOT IN ($1, $2)");
}

#[test]
fn where_in_shortcut() {
    let (sql, params) = pg(User::where_in("id", vec![1, 2, 3]));
    assert_eq!(sql, "SELECT * FROM users WHERE id IN ($1, $2, $3)");
    assert_eq!(params.len(), 3);
}

#[test]
fn where_null_and_not_null() {
    let (sql, _) = pg(User::where_("deleted_at", is_null()));
    assert_eq!(sql, "SELECT * FROM users WHERE deleted_at IS NULL");

    let (sql, _) = pg(User::where_("email", not_null()));
    assert_eq!(sql, "SELECT * FROM users WHERE email IS NOT NULL");

    let (sql, _) = pg(User::where_null("deleted_at"));
    assert_eq!(sql, "SELECT * FROM users WHERE deleted_at IS NULL");

    let (sql, _) = pg(User::where_not_null("email_verified_at"));
    assert_eq!(sql, "SELECT * FROM users WHERE email_verified_at IS NOT NULL");
}

// ---------------------------------------------------------------------------
// Boolean composition
// ---------------------------------------------------------------------------

#[test]
fn and_composes_predicates() {
    let (sql, params) = pg(User::and([pred("a", eq(1)), pred("b", eq(2))]));
    assert_eq!(sql, "SELECT * FROM users WHERE (a = $1 AND b = $2)");
    assert_eq!(params, vec![Value::from(1), Value::from(2)]);
}

#[test]
fn or_composes_predicates() {
    let (sql, params) = pg(User::or([pred("a", eq(1)), pred("b", eq(2))]));
    assert_eq!(sql, "SELECT * FROM users WHERE (a = $1 OR b = $2)");
    assert_eq!(params, vec![Value::from(1), Value::from(2)]);
}

#[test]
fn not_negates_a_predicate() {
    let (sql, params) = pg(User::not(pred("a", eq(1))));
    assert_eq!(sql, "SELECT * FROM users WHERE NOT (a = $1)");
    assert_eq!(params, vec![Value::from(1)]);
}

#[test]
fn nested_and_or() {
    let cond = and([
        pred("a", eq(1)),
        or([pred("b", eq(2)), pred("c", eq(3))]),
    ]);
    let (sql, params) = pg(User::query().where_cond(cond));
    assert_eq!(
        sql,
        "SELECT * FROM users WHERE (a = $1 AND (b = $2 OR c = $3))"
    );
    assert_eq!(params.len(), 3);
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

#[test]
fn select_columns() {
    let (sql, _) = pg(User::select(&["id", "email", "name"]));
    assert_eq!(sql, "SELECT id, email, name FROM users");
}

#[test]
fn distinct() {
    let (sql, _) = pg(User::distinct().select(&["status"]));
    assert_eq!(sql, "SELECT DISTINCT status FROM users");
}

// ---------------------------------------------------------------------------
// Sorting & pagination
// ---------------------------------------------------------------------------

#[test]
fn order_by_desc_and_asc() {
    let (sql, _) = pg(User::order_by("created_at", desc()));
    assert_eq!(sql, "SELECT * FROM users ORDER BY created_at DESC");

    let (sql, _) = pg(User::order_by("name", asc()));
    assert_eq!(sql, "SELECT * FROM users ORDER BY name ASC");
}

#[test]
fn order_by_chains() {
    let (sql, _) = pg(User::order_by("created_at", desc()).order_by("id", asc()));
    assert_eq!(
        sql,
        "SELECT * FROM users ORDER BY created_at DESC, id ASC"
    );
}

#[test]
fn limit_offset() {
    let (sql, _) = pg(User::limit(20).offset(40));
    assert_eq!(sql, "SELECT * FROM users LIMIT 20 OFFSET 40");
}

#[test]
fn paginate() {
    // page=3, per_page=20 -> LIMIT 20 OFFSET 40
    let (sql, _) = pg(User::paginate(3, 20));
    assert_eq!(sql, "SELECT * FROM users LIMIT 20 OFFSET 40");

    // page=1 -> offset 0
    let (sql, _) = pg(User::paginate(1, 50));
    assert_eq!(sql, "SELECT * FROM users LIMIT 50 OFFSET 0");
}

// ---------------------------------------------------------------------------
// Joins
// ---------------------------------------------------------------------------

#[test]
fn inner_join() {
    let (sql, _) = pg(User::join("posts", "users.id", "posts.user_id"));
    assert_eq!(
        sql,
        "SELECT * FROM users INNER JOIN posts ON users.id = posts.user_id"
    );
}

#[test]
fn left_join() {
    let (sql, _) = pg(User::left_join("profiles", "users.id", "profiles.user_id"));
    assert_eq!(
        sql,
        "SELECT * FROM users LEFT JOIN profiles ON users.id = profiles.user_id"
    );
}

// ---------------------------------------------------------------------------
// Aggregates
// ---------------------------------------------------------------------------

#[test]
fn aggregates() {
    assert_eq!(pg(User::sum("amount")).0, "SELECT SUM(amount) FROM users");
    assert_eq!(pg(User::avg("score")).0, "SELECT AVG(score) FROM users");
    assert_eq!(
        pg(User::min("created_at")).0,
        "SELECT MIN(created_at) FROM users"
    );
    assert_eq!(
        pg(User::max("created_at")).0,
        "SELECT MAX(created_at) FROM users"
    );
}

#[test]
fn group_by_having() {
    let (sql, params) = pg(User::query().group_by("status").having("count", gt(5)));
    assert_eq!(
        sql,
        "SELECT * FROM users GROUP BY status HAVING count > $1"
    );
    assert_eq!(params, vec![Value::from(5)]);
}

// ---------------------------------------------------------------------------
// Insert / Update / Delete
// ---------------------------------------------------------------------------

#[test]
fn insert_one() {
    let data = [("email", Value::from("a@b.com")), ("name", Value::from("Ada"))];
    let (sql, params) = pg(User::insert(data));
    assert_eq!(
        sql,
        "INSERT INTO users (email, name) VALUES ($1, $2) RETURNING *"
    );
    assert_eq!(params, vec![Value::from("a@b.com"), Value::from("Ada")]);
}

#[test]
fn insert_many() {
    let rows = vec![
        vec![("email", Value::from("a@b.com")), ("name", Value::from("Ada"))],
        vec![("email", Value::from("c@d.com")), ("name", Value::from("Bo"))],
    ];
    let (sql, params) = pg(User::insert_many(rows));
    assert_eq!(
        sql,
        "INSERT INTO users (email, name) VALUES ($1, $2), ($3, $4) RETURNING *"
    );
    assert_eq!(params.len(), 4);
}

#[test]
fn update_by_id() {
    let data = [("name", Value::from("Ada Lovelace"))];
    let (sql, params) = pg(User::update(1, data));
    assert_eq!(sql, "UPDATE users SET name = $1 WHERE id = $2");
    assert_eq!(params, vec![Value::from("Ada Lovelace"), Value::from(1)]);
}

#[test]
fn update_via_where() {
    let data = [("status", Value::from("active"))];
    let (sql, params) = pg(User::where_("status", eq("inactive")).update(data));
    assert_eq!(sql, "UPDATE users SET status = $1 WHERE status = $2");
    assert_eq!(params, vec![Value::from("active"), Value::from("inactive")]);
}

#[test]
fn delete_by_id() {
    let (sql, params) = pg(User::delete(1));
    assert_eq!(sql, "DELETE FROM users WHERE id = $1");
    assert_eq!(params, vec![Value::from(1)]);
}

#[test]
fn delete_via_where() {
    let (sql, params) = pg(User::where_("status", eq("inactive")).delete());
    assert_eq!(sql, "DELETE FROM users WHERE status = $1");
    assert_eq!(params, vec![Value::from("inactive")]);
}

// ---------------------------------------------------------------------------
// Upsert
// ---------------------------------------------------------------------------

#[test]
fn upsert_on_conflict_do_update() {
    let data = [("email", Value::from("a@b.com")), ("name", Value::from("Ada"))];
    let (sql, _) = pg(User::insert(data).on_conflict("email").update_all());
    assert_eq!(
        sql,
        "INSERT INTO users (email, name) VALUES ($1, $2) ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email, name = EXCLUDED.name RETURNING *"
    );
}

#[test]
fn upsert_on_conflict_do_nothing() {
    let data = [("email", Value::from("a@b.com"))];
    let (sql, _) = pg(User::insert(data).on_conflict("email").do_nothing());
    assert_eq!(
        sql,
        "INSERT INTO users (email) VALUES ($1) ON CONFLICT (email) DO NOTHING"
    );
}

// ---------------------------------------------------------------------------
// Dialects
// ---------------------------------------------------------------------------

#[test]
fn sqlite_uses_qmark_placeholders() {
    let (sql, params) = User::find(1).to_sql(Dialect::Sqlite);
    assert_eq!(sql, "SELECT * FROM users WHERE id = ? LIMIT 1");
    assert_eq!(params, vec![Value::from(1)]);
}

#[test]
fn mysql_uses_qmark_placeholders() {
    let (sql, _) = User::where_("email", eq("a@b.com")).to_sql(Dialect::MySql);
    assert_eq!(sql, "SELECT * FROM users WHERE email = ?");
}

#[test]
fn cockroachdb_renders_as_postgres() {
    let (sql, _) = User::find(1).to_sql(Dialect::CockroachDb);
    assert_eq!(sql, "SELECT * FROM users WHERE id = $1 LIMIT 1");
}

#[test]
fn neon_and_yugabyte_render_as_postgres() {
    let (sql_n, _) = User::find(1).to_sql(Dialect::Neon);
    let (sql_y, _) = User::find(1).to_sql(Dialect::Yugabyte);
    assert_eq!(sql_n, "SELECT * FROM users WHERE id = $1 LIMIT 1");
    assert_eq!(sql_y, "SELECT * FROM users WHERE id = $1 LIMIT 1");
}

#[test]
fn planetscale_tidb_mariadb_render_as_mysql() {
    let (sql_p, _) = User::find(1).to_sql(Dialect::PlanetScale);
    let (sql_t, _) = User::find(1).to_sql(Dialect::TiDb);
    let (sql_m, _) = User::find(1).to_sql(Dialect::MariaDb);
    assert_eq!(sql_p, "SELECT * FROM users WHERE id = ? LIMIT 1");
    assert_eq!(sql_t, "SELECT * FROM users WHERE id = ? LIMIT 1");
    assert_eq!(sql_m, "SELECT * FROM users WHERE id = ? LIMIT 1");
}

#[test]
fn libsql_renders_as_sqlite() {
    let (sql, _) = User::find(1).to_sql(Dialect::LibSql);
    assert_eq!(sql, "SELECT * FROM users WHERE id = ? LIMIT 1");
}

#[test]
fn oracle_uses_colon_placeholders() {
    let (sql, params) = User::where_("email", eq("a@b.com")).to_sql(Dialect::Oracle);
    assert_eq!(sql, "SELECT * FROM users WHERE email = :1");
    assert_eq!(params, vec![Value::from("a@b.com")]);

    // Multiple params get numbered.
    let (sql, _) = User::where_("a", eq(1)).where_("b", eq(2)).to_sql(Dialect::Oracle);
    assert_eq!(sql, "SELECT * FROM users WHERE a = :1 AND b = :2");
}

#[test]
fn mysql_family_does_not_emit_returning() {
    // Plain INSERT in Postgres → RETURNING *; in MySQL → no RETURNING.
    let data = [("email", Value::from("a@b.com"))];
    let (sql_pg, _) = User::insert(data).to_sql(Dialect::Postgres);
    assert_eq!(sql_pg, "INSERT INTO users (email) VALUES ($1) RETURNING *");

    let data = [("email", Value::from("a@b.com"))];
    let (sql_my, _) = User::insert(data).to_sql(Dialect::MySql);
    assert_eq!(sql_my, "INSERT INTO users (email) VALUES (?)");
}

#[test]
fn dialect_family_reports_correctly() {
    use ormx::Family;
    assert_eq!(Dialect::Postgres.family(), Family::Postgres);
    assert_eq!(Dialect::CockroachDb.family(), Family::Postgres);
    assert_eq!(Dialect::MySql.family(), Family::MySql);
    assert_eq!(Dialect::PlanetScale.family(), Family::MySql);
    assert_eq!(Dialect::Sqlite.family(), Family::Sqlite);
    assert_eq!(Dialect::LibSql.family(), Family::Sqlite);
    assert_eq!(Dialect::Oracle.family(), Family::Oracle);
    assert_eq!(Dialect::MsSql.family(), Family::MsSql);
    assert_eq!(Dialect::Db2.family(), Family::Db2);
}

// -- Enterprise dialects: SQL-gen only (no sqlx driver) --------------------

#[test]
fn mssql_uses_at_p_placeholders() {
    let (sql, params) = User::where_("email", eq("a@b.com")).to_sql(Dialect::MsSql);
    assert_eq!(sql, "SELECT * FROM users WHERE email = @p1");
    assert_eq!(params, vec![Value::from("a@b.com")]);

    let (sql, _) = User::where_("a", eq(1)).where_("b", eq(2)).to_sql(Dialect::MsSql);
    assert_eq!(sql, "SELECT * FROM users WHERE a = @p1 AND b = @p2");
}

#[test]
fn mssql_does_not_emit_returning() {
    let data = [("email", Value::from("a@b.com"))];
    let (sql, _) = User::insert(data).to_sql(Dialect::MsSql);
    // No RETURNING on MSSQL — caller has to use OUTPUT clause via raw().
    assert_eq!(sql, "INSERT INTO users (email) VALUES (@p1)");
}

#[test]
fn db2_uses_qmark_placeholders() {
    let (sql, params) = User::where_("email", eq("a@b.com")).to_sql(Dialect::Db2);
    assert_eq!(sql, "SELECT * FROM users WHERE email = ?");
    assert_eq!(params, vec![Value::from("a@b.com")]);
}

#[test]
fn db2_does_not_emit_returning() {
    let data = [("email", Value::from("a@b.com"))];
    let (sql, _) = User::insert(data).to_sql(Dialect::Db2);
    assert_eq!(sql, "INSERT INTO users (email) VALUES (?)");
}

#[test]
fn enterprise_dialects_render_complex_queries() {
    // Make sure the placeholder syntax stays consistent across joined
    // params for the @pN / :N variants.
    use ormx::{between, in_};
    let (sql_ms, _) = User::where_("age", between(18, 65))
        .where_("status", in_(vec!["active", "premium"]))
        .order_by("id", desc())
        .limit(20)
        .to_sql(Dialect::MsSql);
    assert_eq!(
        sql_ms,
        "SELECT * FROM users WHERE age BETWEEN @p1 AND @p2 \
         AND status IN (@p3, @p4) ORDER BY id DESC LIMIT 20"
    );

    let (sql_ora, _) = User::where_("age", between(18, 65))
        .where_("status", in_(vec!["active", "premium"]))
        .order_by("id", desc())
        .limit(20)
        .to_sql(Dialect::Oracle);
    assert_eq!(
        sql_ora,
        "SELECT * FROM users WHERE age BETWEEN :1 AND :2 \
         AND status IN (:3, :4) ORDER BY id DESC LIMIT 20"
    );
}
