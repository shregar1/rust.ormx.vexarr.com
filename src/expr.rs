//! Expressions: operators that pair with a column to form a predicate,
//! plus boolean composition (AND/OR/NOT).
//!
//! Design: `Op` carries the operator + its payload but no column. The column
//! is supplied by `where_(col, op)` or by `pred(col, op)`. `Cond` is the
//! boolean tree built on top of predicates.

use crate::dialect::Dialect;
use crate::value::Value;

#[derive(Debug, Clone)]
pub enum Op {
    Eq(Value),
    Ne(Value),
    Gt(Value),
    Gte(Value),
    Lt(Value),
    Lte(Value),
    Like(Value),
    Ilike(Value),
    Between(Value, Value),
    In(Vec<Value>),
    NotIn(Vec<Value>),
    IsNull,
    NotNull,
}

impl Op {
    /// Render `<col> <op> <placeholders>` and push payload values to `params`.
    pub(crate) fn render(&self, col: &str, dialect: Dialect, params: &mut Vec<Value>) -> String {
        let ph = |v: &Value, params: &mut Vec<Value>| {
            params.push(v.clone());
            dialect.placeholder(params.len())
        };
        match self {
            Op::Eq(v) => format!("{} = {}", col, ph(v, params)),
            Op::Ne(v) => format!("{} <> {}", col, ph(v, params)),
            Op::Gt(v) => format!("{} > {}", col, ph(v, params)),
            Op::Gte(v) => format!("{} >= {}", col, ph(v, params)),
            Op::Lt(v) => format!("{} < {}", col, ph(v, params)),
            Op::Lte(v) => format!("{} <= {}", col, ph(v, params)),
            Op::Like(v) => format!("{} LIKE {}", col, ph(v, params)),
            Op::Ilike(v) => format!("{} ILIKE {}", col, ph(v, params)),
            Op::Between(a, b) => {
                let pa = ph(a, params);
                let pb = ph(b, params);
                format!("{} BETWEEN {} AND {}", col, pa, pb)
            }
            Op::In(vs) => {
                let phs: Vec<String> = vs.iter().map(|v| ph(v, params)).collect();
                format!("{} IN ({})", col, phs.join(", "))
            }
            Op::NotIn(vs) => {
                let phs: Vec<String> = vs.iter().map(|v| ph(v, params)).collect();
                format!("{} NOT IN ({})", col, phs.join(", "))
            }
            Op::IsNull => format!("{} IS NULL", col),
            Op::NotNull => format!("{} IS NOT NULL", col),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Cond {
    Pred { col: String, op: Op },
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
}

impl Cond {
    pub(crate) fn render(&self, dialect: Dialect, params: &mut Vec<Value>) -> String {
        match self {
            Cond::Pred { col, op } => op.render(col, dialect, params),
            Cond::And(items) => {
                let parts: Vec<String> = items.iter().map(|c| c.render(dialect, params)).collect();
                format!("({})", parts.join(" AND "))
            }
            Cond::Or(items) => {
                let parts: Vec<String> = items.iter().map(|c| c.render(dialect, params)).collect();
                format!("({})", parts.join(" OR "))
            }
            Cond::Not(inner) => format!("NOT ({})", inner.render(dialect, params)),
        }
    }
}

// ---------------------------------------------------------------------------
// Public op constructors
// ---------------------------------------------------------------------------

pub fn eq<V: Into<Value>>(v: V) -> Op {
    Op::Eq(v.into())
}
pub fn ne<V: Into<Value>>(v: V) -> Op {
    Op::Ne(v.into())
}
pub fn gt<V: Into<Value>>(v: V) -> Op {
    Op::Gt(v.into())
}
pub fn gte<V: Into<Value>>(v: V) -> Op {
    Op::Gte(v.into())
}
pub fn lt<V: Into<Value>>(v: V) -> Op {
    Op::Lt(v.into())
}
pub fn lte<V: Into<Value>>(v: V) -> Op {
    Op::Lte(v.into())
}
pub fn like<V: Into<Value>>(v: V) -> Op {
    Op::Like(v.into())
}
pub fn ilike<V: Into<Value>>(v: V) -> Op {
    Op::Ilike(v.into())
}
pub fn between<A: Into<Value>, B: Into<Value>>(a: A, b: B) -> Op {
    Op::Between(a.into(), b.into())
}
pub fn in_<V: Into<Value>, I: IntoIterator<Item = V>>(vs: I) -> Op {
    Op::In(vs.into_iter().map(Into::into).collect())
}
pub fn not_in<V: Into<Value>, I: IntoIterator<Item = V>>(vs: I) -> Op {
    Op::NotIn(vs.into_iter().map(Into::into).collect())
}
pub fn is_null() -> Op {
    Op::IsNull
}
pub fn not_null() -> Op {
    Op::NotNull
}

// ---------------------------------------------------------------------------
// Public cond constructors
// ---------------------------------------------------------------------------

pub fn pred(col: impl Into<String>, op: Op) -> Cond {
    Cond::Pred {
        col: col.into(),
        op,
    }
}
pub fn and<I: IntoIterator<Item = Cond>>(conds: I) -> Cond {
    Cond::And(conds.into_iter().collect())
}
pub fn or<I: IntoIterator<Item = Cond>>(conds: I) -> Cond {
    Cond::Or(conds.into_iter().collect())
}
pub fn not(c: Cond) -> Cond {
    Cond::Not(Box::new(c))
}

// ---------------------------------------------------------------------------
// Ordering
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Asc,
    Desc,
}

impl Direction {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Direction::Asc => "ASC",
            Direction::Desc => "DESC",
        }
    }
}

pub fn asc() -> Direction {
    Direction::Asc
}
pub fn desc() -> Direction {
    Direction::Desc
}
