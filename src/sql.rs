//! `ToSql` — every builder implements this and produces a `(sql, params)` pair
//! ready to hand to sqlx.

use crate::dialect::Dialect;
use crate::value::Value;

pub trait ToSql {
    fn to_sql(&self, dialect: Dialect) -> (String, Vec<Value>);
}
