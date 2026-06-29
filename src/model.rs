//! The `Model` trait: implement on a unit struct (or whatever you want to scope
//! the queries to) and you get the full CRUD surface for free.
//!
//! Default-method-only trait so adding helpers here doesn't break existing
//! impls — only `table_name()` is required.

use crate::delete::Delete;
use crate::expr::{Cond, Op};
use crate::insert::Insert;
use crate::select::Select;
use crate::update::Update;
use crate::value::Value;

pub trait Model: Sized {
    fn table_name() -> &'static str;

    // -- Entry points --------------------------------------------------------

    fn query() -> Select<Self> {
        Select::new(Self::table_name())
    }

    fn all() -> Select<Self> {
        Self::query()
    }

    fn first() -> Select<Self> {
        Self::query().limit(1)
    }

    fn find<V: Into<Value>>(id: V) -> Select<Self> {
        Self::query().where_("id", crate::expr::eq(id)).limit(1)
    }

    fn count() -> Select<Self> {
        Select::count_query(Self::table_name())
    }

    fn exists() -> Select<Self> {
        Select::exists_query(Self::table_name())
    }

    fn sum(col: &str) -> Select<Self> {
        Select::aggregate(Self::table_name(), "SUM", col)
    }

    fn avg(col: &str) -> Select<Self> {
        Select::aggregate(Self::table_name(), "AVG", col)
    }

    fn min(col: &str) -> Select<Self> {
        Select::aggregate(Self::table_name(), "MIN", col)
    }

    fn max(col: &str) -> Select<Self> {
        Select::aggregate(Self::table_name(), "MAX", col)
    }

    // -- Filter shortcuts ----------------------------------------------------

    fn where_(col: &str, op: Op) -> Select<Self> {
        Self::query().where_(col, op)
    }

    fn where_in<V: Into<Value>, I: IntoIterator<Item = V>>(col: &str, vs: I) -> Select<Self> {
        Self::query().where_in(col, vs)
    }

    fn where_null(col: &str) -> Select<Self> {
        Self::query().where_null(col)
    }

    fn where_not_null(col: &str) -> Select<Self> {
        Self::query().where_not_null(col)
    }

    fn and<I: IntoIterator<Item = Cond>>(conds: I) -> Select<Self> {
        Self::query().where_cond(crate::expr::and(conds))
    }

    fn or<I: IntoIterator<Item = Cond>>(conds: I) -> Select<Self> {
        Self::query().where_cond(crate::expr::or(conds))
    }

    fn not(cond: Cond) -> Select<Self> {
        Self::query().where_cond(crate::expr::not(cond))
    }

    // -- Projection ----------------------------------------------------------

    fn select(cols: &[&str]) -> Select<Self> {
        Self::query().select(cols)
    }

    fn distinct() -> Select<Self> {
        Self::query().distinct()
    }

    // -- Sorting / pagination ------------------------------------------------

    fn order_by(col: &str, dir: crate::expr::Direction) -> Select<Self> {
        Self::query().order_by(col, dir)
    }

    fn limit(n: u64) -> Select<Self> {
        Self::query().limit(n)
    }

    fn offset(n: u64) -> Select<Self> {
        Self::query().offset(n)
    }

    fn paginate(page: u64, per_page: u64) -> Select<Self> {
        Self::query().paginate(page, per_page)
    }

    // -- Joins ---------------------------------------------------------------

    fn join(table: &str, on_left: &str, on_right: &str) -> Select<Self> {
        Self::query().join(table, on_left, on_right)
    }

    fn left_join(table: &str, on_left: &str, on_right: &str) -> Select<Self> {
        Self::query().left_join(table, on_left, on_right)
    }

    // -- Writes --------------------------------------------------------------

    fn insert<I, K>(data: I) -> Insert<Self>
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        Insert::one(Self::table_name(), data)
    }

    fn insert_many<I, R, K>(rows: I) -> Insert<Self>
    where
        I: IntoIterator<Item = R>,
        R: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        Insert::many(Self::table_name(), rows)
    }

    fn update<I, K, V>(id: V, data: I) -> Update<Self>
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
        V: Into<Value>,
    {
        Update::by_id(Self::table_name(), id, data)
    }

    fn delete<V: Into<Value>>(id: V) -> Delete<Self> {
        Delete::by_id(Self::table_name(), id)
    }
}
