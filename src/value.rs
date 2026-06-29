//! Bindable parameter values.
//!
//! Kept deliberately small — only the JSON-ish primitives that map cleanly to
//! every SQL dialect we care about. Anything fancier should go through the raw
//! escape hatch.

use serde_json::Value as Json;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Text(String),
    Json(Json),
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {
        $(impl From<$t> for Value {
            fn from(v: $t) -> Self { Value::Int(v as i64) }
        })*
    };
}
from_int!(i8, i16, i32, i64, u8, u16, u32);

impl From<u64> for Value {
    fn from(v: u64) -> Self {
        Value::Int(v as i64)
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Real(v as f64)
    }
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Real(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_owned())
    }
}

impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}

impl From<&String> for Value {
    fn from(v: &String) -> Self {
        Value::Text(v.clone())
    }
}

impl From<Json> for Value {
    fn from(v: Json) -> Self {
        Value::Json(v)
    }
}

impl<T> From<Option<T>> for Value
where
    T: Into<Value>,
{
    fn from(opt: Option<T>) -> Self {
        match opt {
            Some(v) => v.into(),
            None => Value::Null,
        }
    }
}
