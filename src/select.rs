//! SELECT builder + COUNT/EXISTS/aggregates.
//!
//! Single type `Select<T>` covers `SELECT *`, projected columns, `COUNT(*)`,
//! `EXISTS(...)`, and aggregate scalars. The `mode` field discriminates.

use std::marker::PhantomData;

use crate::delete::Delete;
use crate::dialect::Dialect;
use crate::expr::{Cond, Direction, Op};
use crate::sql::ToSql;
use crate::update::Update;
use crate::value::Value;

#[derive(Debug, Clone)]
enum Mode {
    Star,
    Cols(Vec<String>),
    CountStar,
    Aggregate(&'static str, String),
    Exists,
}

#[derive(Debug, Clone)]
enum JoinKind {
    Inner,
    Left,
}

#[derive(Debug, Clone)]
struct Join {
    kind: JoinKind,
    table: String,
    on_left: String,
    on_right: String,
}

#[derive(Debug)]
pub struct Select<T> {
    table: &'static str,
    mode: Mode,
    distinct: bool,
    where_clauses: Vec<Cond>,
    order_by: Vec<(String, Direction)>,
    limit: Option<u64>,
    offset: Option<u64>,
    joins: Vec<Join>,
    group_by: Vec<String>,
    having: Vec<Cond>,
    _phantom: PhantomData<T>,
}

impl<T> Select<T> {
    pub(crate) fn new(table: &'static str) -> Self {
        Select {
            table,
            mode: Mode::Star,
            distinct: false,
            where_clauses: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            offset: None,
            joins: Vec::new(),
            group_by: Vec::new(),
            having: Vec::new(),
            _phantom: PhantomData,
        }
    }

    pub(crate) fn count_query(table: &'static str) -> Self {
        let mut s = Self::new(table);
        s.mode = Mode::CountStar;
        s
    }

    pub(crate) fn exists_query(table: &'static str) -> Self {
        let mut s = Self::new(table);
        s.mode = Mode::Exists;
        s
    }

    pub(crate) fn aggregate(table: &'static str, func: &'static str, col: &str) -> Self {
        let mut s = Self::new(table);
        s.mode = Mode::Aggregate(func, col.to_owned());
        s
    }

    // -- Column projection ----------------------------------------------------

    pub fn select(mut self, cols: &[&str]) -> Self {
        self.mode = Mode::Cols(cols.iter().map(|c| (*c).to_owned()).collect());
        self
    }

    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    // -- WHERE ---------------------------------------------------------------

    pub fn where_(mut self, col: &str, op: Op) -> Self {
        self.where_clauses.push(Cond::Pred {
            col: col.to_owned(),
            op,
        });
        self
    }

    pub fn where_cond(mut self, cond: Cond) -> Self {
        self.where_clauses.push(cond);
        self
    }

    pub fn where_in<V: Into<Value>, I: IntoIterator<Item = V>>(self, col: &str, vs: I) -> Self {
        self.where_(col, crate::expr::in_(vs))
    }

    pub fn where_null(self, col: &str) -> Self {
        self.where_(col, crate::expr::is_null())
    }

    pub fn where_not_null(self, col: &str) -> Self {
        self.where_(col, crate::expr::not_null())
    }

    // -- ORDER / LIMIT -------------------------------------------------------

    pub fn order_by(mut self, col: &str, dir: Direction) -> Self {
        self.order_by.push((col.to_owned(), dir));
        self
    }

    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    pub fn paginate(mut self, page: u64, per_page: u64) -> Self {
        let page = page.max(1);
        self.limit = Some(per_page);
        self.offset = Some((page - 1) * per_page);
        self
    }

    // -- JOINS ---------------------------------------------------------------

    pub fn join(mut self, table: &str, on_left: &str, on_right: &str) -> Self {
        self.joins.push(Join {
            kind: JoinKind::Inner,
            table: table.to_owned(),
            on_left: on_left.to_owned(),
            on_right: on_right.to_owned(),
        });
        self
    }

    pub fn left_join(mut self, table: &str, on_left: &str, on_right: &str) -> Self {
        self.joins.push(Join {
            kind: JoinKind::Left,
            table: table.to_owned(),
            on_left: on_left.to_owned(),
            on_right: on_right.to_owned(),
        });
        self
    }

    // -- GROUP BY / HAVING ---------------------------------------------------

    pub fn group_by(mut self, col: &str) -> Self {
        self.group_by.push(col.to_owned());
        self
    }

    pub fn having(mut self, col: &str, op: Op) -> Self {
        self.having.push(Cond::Pred {
            col: col.to_owned(),
            op,
        });
        self
    }

    // -- Mutation transitions ------------------------------------------------

    pub fn update<I, K>(self, data: I) -> Update<T>
    where
        I: IntoIterator<Item = (K, Value)>,
        K: Into<String>,
    {
        Update::from_select(self.table, self.where_clauses, data)
    }

    pub fn delete(self) -> Delete<T> {
        Delete::from_select(self.table, self.where_clauses)
    }
}

// ---------------------------------------------------------------------------
// SQL rendering
// ---------------------------------------------------------------------------

impl<T> ToSql for Select<T> {
    fn to_sql(&self, dialect: Dialect) -> (String, Vec<Value>) {
        let mut params: Vec<Value> = Vec::new();

        // EXISTS wraps the entire query.
        if matches!(self.mode, Mode::Exists) {
            let inner = self.render_body(dialect, &mut params, /* select_clause */ "SELECT 1");
            return (format!("SELECT EXISTS({})", inner), params);
        }

        let select_clause = match &self.mode {
            Mode::Star => "SELECT *".to_owned(),
            Mode::Cols(cs) => {
                let d = if self.distinct { "DISTINCT " } else { "" };
                format!("SELECT {}{}", d, cs.join(", "))
            }
            Mode::CountStar => "SELECT COUNT(*)".to_owned(),
            Mode::Aggregate(fun, col) => format!("SELECT {}({})", fun, col),
            Mode::Exists => unreachable!(),
        };

        let body = self.render_body(dialect, &mut params, &select_clause);
        (body, params)
    }
}

impl<T> Select<T> {
    fn render_body(&self, dialect: Dialect, params: &mut Vec<Value>, select_clause: &str) -> String {
        let mut sql = format!("{} FROM {}", select_clause, self.table);

        for j in &self.joins {
            let kw = match j.kind {
                JoinKind::Inner => "INNER JOIN",
                JoinKind::Left => "LEFT JOIN",
            };
            sql.push_str(&format!(
                " {} {} ON {} = {}",
                kw, j.table, j.on_left, j.on_right
            ));
        }

        if !self.where_clauses.is_empty() {
            let parts: Vec<String> = self
                .where_clauses
                .iter()
                .map(|c| c.render(dialect, params))
                .collect();
            sql.push_str(" WHERE ");
            sql.push_str(&parts.join(" AND "));
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            sql.push_str(&self.group_by.join(", "));
        }

        if !self.having.is_empty() {
            let parts: Vec<String> = self
                .having
                .iter()
                .map(|c| c.render(dialect, params))
                .collect();
            sql.push_str(" HAVING ");
            sql.push_str(&parts.join(" AND "));
        }

        if !self.order_by.is_empty() {
            let parts: Vec<String> = self
                .order_by
                .iter()
                .map(|(c, d)| format!("{} {}", c, d.as_sql()))
                .collect();
            sql.push_str(" ORDER BY ");
            sql.push_str(&parts.join(", "));
        }

        if let Some(l) = self.limit {
            sql.push_str(&format!(" LIMIT {}", l));
        }
        if let Some(o) = self.offset {
            sql.push_str(&format!(" OFFSET {}", o));
        }

        sql
    }
}
