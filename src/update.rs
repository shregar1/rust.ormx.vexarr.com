//! UPDATE builder. SET assignments render first (so their params come before
//! the WHERE params), then WHERE.

use std::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{Cond, Op};
use crate::sql::ToSql;
use crate::value::Value;

#[derive(Debug)]
pub struct Update<T> {
    table: &'static str,
    set: Vec<(String, Value)>,
    where_clauses: Vec<Cond>,
    _phantom: PhantomData<T>,
}

impl<T> Update<T> {
    pub(crate) fn by_id<I, K, V>(table: &'static str, id: V, data: I) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
        V: Into<Value>,
    {
        let set: Vec<(String, Value)> =
            data.into_iter().map(|(k, v)| (k.into(), v)).collect();
        Update {
            table,
            set,
            where_clauses: vec![Cond::Pred {
                col: "id".to_owned(),
                op: Op::Eq(id.into()),
            }],
            _phantom: PhantomData,
        }
    }

    pub(crate) fn from_select<I, K>(
        table: &'static str,
        where_clauses: Vec<Cond>,
        data: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        let set: Vec<(String, Value)> =
            data.into_iter().map(|(k, v)| (k.into(), v)).collect();
        Update {
            table,
            set,
            where_clauses,
            _phantom: PhantomData,
        }
    }
}

impl<T> ToSql for Update<T> {
    fn to_sql(&self, dialect: Dialect) -> (String, Vec<Value>) {
        let mut params: Vec<Value> = Vec::new();

        let mut assigns = Vec::with_capacity(self.set.len());
        for (col, v) in &self.set {
            params.push(v.clone());
            assigns.push(format!("{} = {}", col, dialect.placeholder(params.len())));
        }

        let mut sql = format!("UPDATE {} SET {}", self.table, assigns.join(", "));

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
