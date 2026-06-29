//! DELETE builder.

use std::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{Cond, Op};
use crate::sql::ToSql;
use crate::value::Value;

#[derive(Debug)]
pub struct Delete<T> {
    table: &'static str,
    where_clauses: Vec<Cond>,
    _phantom: PhantomData<T>,
}

impl<T> Delete<T> {
    pub(crate) fn by_id<V: Into<Value>>(table: &'static str, id: V) -> Self {
        Delete {
            table,
            where_clauses: vec![Cond::Pred {
                col: "id".to_owned(),
                op: Op::Eq(id.into()),
            }],
            _phantom: PhantomData,
        }
    }

    pub(crate) fn from_select(table: &'static str, where_clauses: Vec<Cond>) -> Self {
        Delete {
            table,
            where_clauses,
            _phantom: PhantomData,
        }
    }
}

impl<T> ToSql for Delete<T> {
    fn to_sql(&self, dialect: Dialect) -> (String, Vec<Value>) {
        let mut params: Vec<Value> = Vec::new();
        let mut sql = format!("DELETE FROM {}", self.table);

        if !self.where_clauses.is_empty() {
            let parts: Vec<String> = self
                .where_clauses
                .iter()
                .map(|c| c.render(dialect, &mut params))
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&parts.join(" AND "));
        }

        (sql, params)
    }
}
