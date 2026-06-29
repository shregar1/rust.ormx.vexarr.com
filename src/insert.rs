//! INSERT builder, including ON CONFLICT (upsert) transitions.

use std::marker::PhantomData;

use crate::dialect::Dialect;
use crate::sql::ToSql;
use crate::value::Value;

#[derive(Debug, Clone)]
enum Conflict {
    DoNothing { target: String },
    UpdateAll { target: String },
}

#[derive(Debug)]
pub struct Insert<T> {
    table: &'static str,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    conflict: Option<Conflict>,
    _phantom: PhantomData<T>,
}

impl<T> Insert<T> {
    pub(crate) fn one<I, K>(table: &'static str, data: I) -> Self
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        let mut columns = Vec::new();
        let mut row = Vec::new();
        for (k, v) in data {
            columns.push(k.into());
            row.push(v);
        }
        Insert {
            table,
            columns,
            rows: vec![row],
            conflict: None,
            _phantom: PhantomData,
        }
    }

    pub(crate) fn many<I, R, K>(table: &'static str, rows: I) -> Self
    where
        I: IntoIterator<Item = R>,
        R: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        let mut columns: Vec<String> = Vec::new();
        let mut out_rows: Vec<Vec<Value>> = Vec::new();
        for (i, row) in rows.into_iter().enumerate() {
            let mut values = Vec::new();
            if i == 0 {
                for (k, v) in row {
                    columns.push(k.into());
                    values.push(v);
                }
            } else {
                for (_, v) in row {
                    values.push(v);
                }
            }
            out_rows.push(values);
        }
        Insert {
            table,
            columns,
            rows: out_rows,
            conflict: None,
            _phantom: PhantomData,
        }
    }

    pub fn on_conflict(self, target: &str) -> InsertOnConflict<T> {
        InsertOnConflict {
            inner: self,
            target: target.to_owned(),
        }
    }
}

pub struct InsertOnConflict<T> {
    inner: Insert<T>,
    target: String,
}

impl<T> InsertOnConflict<T> {
    pub fn do_nothing(mut self) -> Insert<T> {
        self.inner.conflict = Some(Conflict::DoNothing {
            target: self.target,
        });
        self.inner
    }

    pub fn update_all(mut self) -> Insert<T> {
        self.inner.conflict = Some(Conflict::UpdateAll {
            target: self.target,
        });
        self.inner
    }
}

impl<T> ToSql for Insert<T> {
    fn to_sql(&self, dialect: Dialect) -> (String, Vec<Value>) {
        let mut params: Vec<Value> = Vec::new();
        let cols = self.columns.join(", ");

        // Build VALUES (...), (...), ...
        let mut row_parts: Vec<String> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let mut placeholders = Vec::with_capacity(row.len());
            for v in row {
                params.push(v.clone());
                placeholders.push(dialect.placeholder(params.len()));
            }
            row_parts.push(format!("({})", placeholders.join(", ")));
        }

        let mut sql = format!(
            "INSERT INTO {} ({}) VALUES {}",
            self.table,
            cols,
            row_parts.join(", ")
        );

        let mut emit_returning = true;
        if let Some(conflict) = &self.conflict {
            match conflict {
                Conflict::DoNothing { target } => {
                    sql.push_str(&format!(" ON CONFLICT ({}) DO NOTHING", target));
                    emit_returning = false;
                }
                Conflict::UpdateAll { target } => {
                    let assigns: Vec<String> = self
                        .columns
                        .iter()
                        .map(|c| format!("{} = EXCLUDED.{}", c, c))
                        .collect();
                    sql.push_str(&format!(
                        " ON CONFLICT ({}) DO UPDATE SET {}",
                        target,
                        assigns.join(", ")
                    ));
                }
            }
        }

        if emit_returning && dialect.supports_returning() {
            sql.push_str(" RETURNING *");
        }

        (sql, params)
    }
}
