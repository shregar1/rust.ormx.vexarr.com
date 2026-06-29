//! Raw SQL escape hatch. Bypasses the builder entirely — you own the SQL,
//! ormx just bundles it with params so the executor binds them the same way.

use crate::dialect::Dialect;
use crate::sql::ToSql;
use crate::value::Value;

#[derive(Debug, Clone)]
pub struct Raw {
    sql: String,
    params: Vec<Value>,
}

pub fn raw<S, I, V>(sql: S, params: I) -> Raw
where
    S: Into<String>,
    I: IntoIterator<Item = V>,
    V: Into<Value>,
{
    Raw {
        sql: sql.into(),
        params: params.into_iter().map(Into::into).collect(),
    }
}

impl ToSql for Raw {
    fn to_sql(&self, _dialect: Dialect) -> (String, Vec<Value>) {
        // Raw SQL is dialect-pinned by the caller — we don't rewrite placeholders.
        (self.sql.clone(), self.params.clone())
    }
}
