//! Schema introspection → typed Rust models (Phase 1, Postgres only).
//!
//! Reads `information_schema` to discover tables, columns, and primary
//! keys, then emits Rust source: a `struct` per table with a derived
//! `sqlx::FromRow` and an `impl ormx::Model` block. The output is
//! string-ready; you write it to disk, format with `rustfmt`, and
//! commit.
//!
//! What it captures today:
//! - All `BASE TABLE` rows under the chosen schema (filterable by name).
//! - Per-column: name, Postgres type, nullable, primary-key flag.
//! - Maps the common Postgres types to their canonical Rust shapes.
//!
//! What it leaves on the floor (Phase 2 fodder):
//! - Foreign keys → typed relations.
//! - Unique / index metadata.
//! - User-defined enums (currently emits a TODO comment).
//! - Per-language output (Python, Node, Go, Java, Ruby).
//! - A `bin/ormx-generate` CLI binary.
//!
//! ```ignore
//! use ormx::codegen;
//! let pool = sqlx::postgres::PgPoolOptions::new().connect(dsn).await?;
//! let tables = codegen::introspect(&pool, "public", None).await?;
//! for t in &tables {
//!     println!("{}", codegen::render_rust(t));
//! }
//! ```

use sqlx::PgPool;

use crate::Result;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One column on a table, as introspected from `information_schema`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Postgres type name as reported by `data_type` (e.g. `"integer"`,
    /// `"timestamp with time zone"`). For user-defined types this is
    /// `"USER-DEFINED"`; the actual name lives in `udt_name`.
    pub pg_type: String,
    /// `udt_name` from `information_schema.columns`. For built-ins this
    /// matches what Postgres calls the type internally (`int4`, `text`,
    /// `timestamptz`); for user-defined types this is the type name
    /// (`my_enum`).
    pub udt_name: String,
    pub nullable: bool,
    pub is_primary_key: bool,
}

/// One table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub schema: String,
    pub name: String,
    pub columns: Vec<Column>,
}

// ---------------------------------------------------------------------------
// Introspection
// ---------------------------------------------------------------------------

/// Read every `BASE TABLE` in `schema`, optionally filtered to a subset
/// by name. Returns tables sorted by name with columns in ordinal
/// position.
pub async fn introspect(
    pool: &PgPool,
    schema: &str,
    only: Option<&[&str]>,
) -> Result<Vec<Table>> {
    // 1. Table names
    let mut q = String::from(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = $1 AND table_type = 'BASE TABLE'",
    );
    if only.is_some() {
        q.push_str(" AND table_name = ANY($2)");
    }
    q.push_str(" ORDER BY table_name");
    let mut query = sqlx::query_as::<_, (String,)>(&q).bind(schema);
    if let Some(names) = only {
        let owned: Vec<String> = names.iter().map(|s| (*s).to_owned()).collect();
        query = query.bind(owned);
    }
    let table_rows = query.fetch_all(pool).await?;

    // 2. Columns for every table in one shot. (Cheaper than N×fetch on
    //    big schemas; we filter the in-memory list down per table.)
    let table_names: Vec<String> = table_rows.iter().map(|(n,)| n.clone()).collect();
    if table_names.is_empty() {
        return Ok(Vec::new());
    }
    let col_rows: Vec<(String, String, i32, String, String, String)> = sqlx::query_as(
        "SELECT table_name, column_name, ordinal_position, \
                data_type, udt_name, is_nullable \
         FROM information_schema.columns \
         WHERE table_schema = $1 AND table_name = ANY($2) \
         ORDER BY table_name, ordinal_position",
    )
    .bind(schema)
    .bind(&table_names)
    .fetch_all(pool)
    .await?;

    // 3. Primary-key columns across all selected tables, also in one shot.
    let pk_rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT kcu.table_name, kcu.column_name \
         FROM information_schema.table_constraints AS tc \
         JOIN information_schema.key_column_usage AS kcu \
           ON tc.constraint_name = kcu.constraint_name \
          AND tc.table_schema = kcu.table_schema \
         WHERE tc.constraint_type = 'PRIMARY KEY' \
           AND tc.table_schema = $1 \
           AND tc.table_name = ANY($2)",
    )
    .bind(schema)
    .bind(&table_names)
    .fetch_all(pool)
    .await?;

    use std::collections::HashSet;
    let pk_set: HashSet<(String, String)> = pk_rows.into_iter().collect();

    let mut out = Vec::with_capacity(table_rows.len());
    for (tname,) in &table_rows {
        let columns: Vec<Column> = col_rows
            .iter()
            .filter(|(t, ..)| t == tname)
            .map(|(_, col, _, data_type, udt, is_null)| Column {
                name: col.clone(),
                pg_type: data_type.clone(),
                udt_name: udt.clone(),
                nullable: is_null == "YES",
                is_primary_key: pk_set.contains(&(tname.clone(), col.clone())),
            })
            .collect();
        out.push(Table { schema: schema.to_owned(), name: tname.clone(), columns });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Type mapping (Postgres → Rust)
// ---------------------------------------------------------------------------

/// Map a Postgres `(data_type, udt_name)` pair to a Rust type. Returns
/// `None` for types we don't know how to represent — the renderer will
/// emit `serde_json::Value` with a `TODO` comment for those.
pub fn pg_to_rust(data_type: &str, udt_name: &str) -> Option<&'static str> {
    // Try `data_type` first (more specific in some cases), fall back to
    // `udt_name` (uses Postgres' internal names like `int4`, `timestamptz`).
    let key = data_type.to_lowercase();
    let udt = udt_name.to_lowercase();
    match key.as_str() {
        "smallint" => Some("i16"),
        "integer" => Some("i32"),
        "bigint" => Some("i64"),
        "real" => Some("f32"),
        "double precision" => Some("f64"),
        "numeric" | "decimal" => Some("rust_decimal::Decimal"),
        "boolean" => Some("bool"),
        "text" | "character varying" | "character" | "varchar" | "char" | "citext" => Some("String"),
        "bytea" => Some("Vec<u8>"),
        "uuid" => Some("uuid::Uuid"),
        "json" | "jsonb" => Some("serde_json::Value"),
        "timestamp without time zone" => Some("chrono::NaiveDateTime"),
        "timestamp with time zone" => Some("chrono::DateTime<chrono::Utc>"),
        "date" => Some("chrono::NaiveDate"),
        "time without time zone" => Some("chrono::NaiveTime"),
        "interval" => Some("sqlx::postgres::types::PgInterval"),
        // Arrays of the common scalars
        "array" => match udt.as_str() {
            "_int2" => Some("Vec<i16>"),
            "_int4" => Some("Vec<i32>"),
            "_int8" => Some("Vec<i64>"),
            "_float4" => Some("Vec<f32>"),
            "_float8" => Some("Vec<f64>"),
            "_text" | "_varchar" => Some("Vec<String>"),
            "_bool" => Some("Vec<bool>"),
            _ => None,
        },
        _ => match udt.as_str() {
            // udt_name fallbacks for things data_type spells differently
            "int2" => Some("i16"),
            "int4" => Some("i32"),
            "int8" => Some("i64"),
            "float4" => Some("f32"),
            "float8" => Some("f64"),
            "timestamptz" => Some("chrono::DateTime<chrono::Utc>"),
            "timestamp" => Some("chrono::NaiveDateTime"),
            _ => None,
        },
    }
}

/// Resolve a column to the final Rust type token (wraps in `Option<…>`
/// for nullable, falls back to `serde_json::Value` with a TODO note for
/// types we don't recognise yet).
pub fn rust_type_for(col: &Column) -> String {
    let base = match pg_to_rust(&col.pg_type, &col.udt_name) {
        Some(t) => t.to_owned(),
        None => format!(
            "serde_json::Value /* TODO: unmapped pg type {}/{} */",
            col.pg_type, col.udt_name
        ),
    };
    if col.nullable {
        format!("Option<{base}>")
    } else {
        base
    }
}

// ---------------------------------------------------------------------------
// Rust rendering
// ---------------------------------------------------------------------------

/// Render a `Table` as Rust source — struct + `Model` impl. The output
/// is intentionally close to what a human would type: derives + pub
/// fields + a one-line impl. Pipe through `rustfmt` for canonical
/// spacing; the renderer doesn't try to match `rustfmt` exactly.
pub fn render_rust(table: &Table) -> String {
    let struct_name = to_pascal_case(&table.name);
    let mut out = String::with_capacity(256 + table.columns.len() * 64);

    out.push_str("#[derive(Debug, Clone, sqlx::FromRow)]\n");
    out.push_str(&format!("pub struct {struct_name} {{\n"));
    for col in &table.columns {
        if col.is_primary_key {
            out.push_str("    // primary key\n");
        }
        let ty = rust_type_for(col);
        // Reserved-keyword guard: if the column name collides with a Rust
        // keyword, prepend r# so the field name stays the column name.
        let field = if is_reserved(&col.name) {
            format!("r#{}", col.name)
        } else {
            col.name.clone()
        };
        out.push_str(&format!("    pub {field}: {ty},\n"));
    }
    out.push_str("}\n\n");

    out.push_str(&format!("impl ormx::Model for {struct_name} {{\n"));
    out.push_str(&format!(
        "    fn table_name() -> &'static str {{ \"{}\" }}\n",
        table.name
    ));
    out.push_str("}\n");
    out
}

/// Render every table in a single source string with a module-level
/// header. Convenience for "write the whole thing to one file."
pub fn render_rust_all(tables: &[Table]) -> String {
    let mut out = String::from(
        "// Generated by `ormx::codegen`. Edit by hand at your own risk.\n\
         #![allow(dead_code, non_snake_case)]\n\n",
    );
    for (i, t) in tables.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&render_rust(t));
    }
    out
}

// ---------------------------------------------------------------------------
// Other-language rendering
// ---------------------------------------------------------------------------
//
// Each renderer is intentionally independent — no shared trait, just plain
// fns. They all consume the same `Table` shape but emit idiomatic source
// for their target ecosystem. Type maps differ per language because each
// ecosystem has its own canonical "what's the cheapest legal type for X"
// answer (e.g. Postgres bigint → Python int, Node bigint, Go int64, Ruby
// nothing, Java long).

/// Map Postgres → Python type token (PEP 484). Returns `Any` for
/// unknown so the emitted source still type-checks under most tools.
pub fn pg_to_python(data_type: &str, udt_name: &str) -> &'static str {
    let key = data_type.to_lowercase();
    let udt = udt_name.to_lowercase();
    match key.as_str() {
        "smallint" | "integer" | "bigint" => "int",
        "real" | "double precision" => "float",
        "numeric" | "decimal" => "Decimal",
        "boolean" => "bool",
        "text" | "character varying" | "character" | "varchar" | "char" | "citext" => "str",
        "bytea" => "bytes",
        "uuid" => "UUID",
        "json" | "jsonb" => "dict",
        "timestamp without time zone" | "timestamp with time zone" => "datetime",
        "date" => "date",
        "time without time zone" => "time",
        _ => match udt.as_str() {
            "int2" | "int4" | "int8" => "int",
            "float4" | "float8" => "float",
            "timestamptz" | "timestamp" => "datetime",
            _ => "Any",
        },
    }
}

/// Map Postgres → TypeScript type token. `bigint` for INT8 because
/// JavaScript loses precision above 2^53; everything else fits a
/// `number`.
pub fn pg_to_ts(data_type: &str, udt_name: &str) -> &'static str {
    let key = data_type.to_lowercase();
    let udt = udt_name.to_lowercase();
    match key.as_str() {
        "bigint" => "bigint",
        "smallint" | "integer" => "number",
        "real" | "double precision" | "numeric" | "decimal" => "number",
        "boolean" => "boolean",
        "text" | "character varying" | "character" | "varchar" | "char" | "citext" => "string",
        "bytea" => "Buffer",
        "uuid" => "string",
        "json" | "jsonb" => "unknown",
        "timestamp without time zone" | "timestamp with time zone" | "date" => "Date",
        "time without time zone" => "string",
        _ => match udt.as_str() {
            "int8" => "bigint",
            "int2" | "int4" => "number",
            "float4" | "float8" => "number",
            "timestamptz" | "timestamp" => "Date",
            _ => "unknown",
        },
    }
}

/// Map Postgres → Go type token. Nullable columns become a pointer
/// (`*int64`) in idiomatic Go rather than `sql.NullInt64`.
pub fn pg_to_go(data_type: &str, udt_name: &str) -> &'static str {
    let key = data_type.to_lowercase();
    let udt = udt_name.to_lowercase();
    match key.as_str() {
        "smallint" => "int16",
        "integer" => "int32",
        "bigint" => "int64",
        "real" => "float32",
        "double precision" | "numeric" | "decimal" => "float64",
        "boolean" => "bool",
        "text" | "character varying" | "character" | "varchar" | "char" | "citext" => "string",
        "bytea" => "[]byte",
        "uuid" => "string",
        "json" | "jsonb" => "json.RawMessage",
        "timestamp without time zone" | "timestamp with time zone" | "date" => "time.Time",
        "time without time zone" => "string",
        _ => match udt.as_str() {
            "int2" => "int16",
            "int4" => "int32",
            "int8" => "int64",
            "float4" => "float32",
            "float8" => "float64",
            "timestamptz" | "timestamp" => "time.Time",
            _ => "interface{}",
        },
    }
}

/// Map Postgres → Java type token. Always returns the boxed form
/// (`Long`, `Integer`, `Boolean`) so the same name works for nullable
/// + non-nullable; primitive shorthand happens only in record fields
/// when we know it's NOT NULL.
pub fn pg_to_java_boxed(data_type: &str, udt_name: &str) -> &'static str {
    let key = data_type.to_lowercase();
    let udt = udt_name.to_lowercase();
    match key.as_str() {
        "smallint" => "Short",
        "integer" => "Integer",
        "bigint" => "Long",
        "real" => "Float",
        "double precision" | "numeric" | "decimal" => "Double",
        "boolean" => "Boolean",
        "text" | "character varying" | "character" | "varchar" | "char" | "citext" => "String",
        "bytea" => "byte[]",
        "uuid" => "java.util.UUID",
        "json" | "jsonb" => "String",
        "timestamp with time zone" => "java.time.OffsetDateTime",
        "timestamp without time zone" => "java.time.LocalDateTime",
        "date" => "java.time.LocalDate",
        "time without time zone" => "java.time.LocalTime",
        _ => match udt.as_str() {
            "int2" => "Short",
            "int4" => "Integer",
            "int8" => "Long",
            "float4" => "Float",
            "float8" => "Double",
            "timestamptz" => "java.time.OffsetDateTime",
            "timestamp" => "java.time.LocalDateTime",
            _ => "Object",
        },
    }
}

/// Java primitive form when we know the column is NOT NULL. Falls
/// through to the boxed type for everything that doesn't have a
/// primitive form (`String`, `byte[]`, `UUID`, …).
pub fn pg_to_java_primitive(data_type: &str, udt_name: &str) -> &'static str {
    match pg_to_java_boxed(data_type, udt_name) {
        "Short" => "short",
        "Integer" => "int",
        "Long" => "long",
        "Float" => "float",
        "Double" => "double",
        "Boolean" => "boolean",
        other => other,
    }
}

/// Render a Python model. Uses the `Model` base from `ormx-python`
/// with `PrimaryKey()` markers on PK columns and `Optional[T]` for
/// nullable ones. Imports are emitted alongside (caller can dedupe).
pub fn render_python(table: &Table) -> String {
    let class_name = to_pascal_case(&table.name);
    let mut out = String::with_capacity(256 + table.columns.len() * 64);
    out.push_str(&format!("class {class_name}(Model):\n"));
    out.push_str(&format!("    table_name = \"{}\"\n\n", table.name));
    for col in &table.columns {
        let py = pg_to_python(&col.pg_type, &col.udt_name);
        let ty = if col.nullable {
            format!("Optional[{py}]")
        } else {
            py.to_owned()
        };
        let suffix = if col.is_primary_key { " = PrimaryKey()" } else { "" };
        out.push_str(&format!("    {}: {ty}{suffix}\n", col.name));
    }
    out
}

/// Render a TypeScript / Node model class for the ormx-node SDK.
/// Uses `declare` so the field types are erased at runtime — keeps
/// init cost zero.
pub fn render_node(table: &Table) -> String {
    let class_name = to_pascal_case(&table.name);
    let mut out = String::with_capacity(256 + table.columns.len() * 64);
    out.push_str(&format!("export class {class_name} extends Model {{\n"));
    out.push_str(&format!("  static tableName = \"{}\";\n", table.name));
    for col in &table.columns {
        let ts = pg_to_ts(&col.pg_type, &col.udt_name);
        let ty = if col.nullable { format!("{ts} | null") } else { ts.to_owned() };
        let pk_note = if col.is_primary_key { "  // primary key\n" } else { "" };
        out.push_str(pk_note);
        out.push_str(&format!("  declare {}: {ty};\n", col.name));
    }
    out.push_str("}\n");
    out
}

/// Render a Go struct + `TableName()` method for the ormx-go SDK.
/// Nullable columns become pointer types (`*int64`) — idiomatic Go
/// for "may be NULL".
pub fn render_go(table: &Table) -> String {
    let struct_name = to_pascal_case(&table.name);
    let mut out = String::with_capacity(256 + table.columns.len() * 64);
    out.push_str(&format!("type {struct_name} struct {{\n"));
    for col in &table.columns {
        let go = pg_to_go(&col.pg_type, &col.udt_name);
        let ty = if col.nullable { format!("*{go}") } else { go.to_owned() };
        let field = to_pascal_case(&col.name);
        if col.is_primary_key {
            out.push_str("    // primary key\n");
        }
        out.push_str(&format!("    {field} {ty} `db:\"{}\"`\n", col.name));
    }
    out.push_str("}\n\n");
    out.push_str(&format!("func ({struct_name}) TableName() string {{ return \"{}\" }}\n", table.name));
    out
}

/// Render a Ruby class for the ormx-ruby SDK. Ruby is dynamic so we
/// emit `attributes` symbol list — no per-field types.
pub fn render_ruby(table: &Table) -> String {
    let class_name = to_pascal_case(&table.name);
    let mut out = String::with_capacity(128 + table.columns.len() * 16);
    out.push_str(&format!("class {class_name} < Ormx::Model\n"));
    out.push_str(&format!("  table_name \"{}\"\n", table.name));
    out.push_str("  attributes ");
    let syms: Vec<String> = table.columns.iter().map(|c| format!(":{}", c.name)).collect();
    out.push_str(&syms.join(", "));
    out.push('\n');
    // Mark PKs in a comment for human readers.
    let pks: Vec<&str> = table.columns.iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    if !pks.is_empty() {
        out.push_str(&format!("  # primary key: {}\n", pks.join(", ")));
    }
    out.push_str("end\n");
    out
}

/// Render a Java record for the ormx-java SDK. Records are the
/// canonical "value-shaped immutable" in modern Java. Primitive types
/// used when the column is NOT NULL; boxed types otherwise (Java
/// records can't have nullable primitives).
pub fn render_java(table: &Table) -> String {
    let class_name = to_pascal_case(&table.name);
    let mut out = String::with_capacity(256 + table.columns.len() * 64);
    let pks: Vec<&str> = table.columns.iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    if !pks.is_empty() {
        out.push_str(&format!("// primary key: {}\n", pks.join(", ")));
    }
    out.push_str(&format!("public record {class_name}(\n"));
    for (i, col) in table.columns.iter().enumerate() {
        let ty = if col.nullable {
            pg_to_java_boxed(&col.pg_type, &col.udt_name).to_owned()
        } else {
            pg_to_java_primitive(&col.pg_type, &col.udt_name).to_owned()
        };
        let comma = if i + 1 < table.columns.len() { "," } else { "" };
        let field = to_java_camel_case(&col.name);
        out.push_str(&format!("    {ty} {field}{comma}\n"));
    }
    out.push_str(") {}\n");
    out
}

/// `audit_log_entry` → `auditLogEntry`.
fn to_java_camel_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut next_upper = false;
    for (i, c) in s.chars().enumerate() {
        if c == '_' || c == '-' || c == ' ' {
            next_upper = true;
        } else if c.is_alphanumeric() {
            if next_upper {
                for u in c.to_uppercase() {
                    out.push(u);
                }
                next_upper = false;
            } else if i == 0 {
                for u in c.to_lowercase() {
                    out.push(u);
                }
            } else {
                out.push(c);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `users` → `Users`, `audit_log_entry` → `AuditLogEntry`.
/// Anything that isn't `[a-zA-Z0-9_]` gets dropped.
pub(crate) fn to_pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut next_upper = true;
    for c in s.chars() {
        if c == '_' || c == '-' || c == ' ' {
            next_upper = true;
        } else if c.is_alphanumeric() {
            if next_upper {
                for u in c.to_uppercase() {
                    out.push(u);
                }
                next_upper = false;
            } else {
                out.push(c);
            }
        }
    }
    out
}

fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum" | "extern"
        | "false" | "fn" | "for" | "if" | "impl" | "in" | "let" | "loop" | "match"
        | "mod" | "move" | "mut" | "pub" | "ref" | "return" | "self" | "Self"
        | "static" | "struct" | "super" | "trait" | "true" | "type" | "unsafe"
        | "use" | "where" | "while" | "async" | "await" | "dyn" | "abstract"
        | "become" | "box" | "do" | "final" | "macro" | "override" | "priv"
        | "typeof" | "unsized" | "virtual" | "yield" | "try"
    )
}

// ---------------------------------------------------------------------------
// Unit tests — pure mapping + rendering; no DB.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, udt: &str, nullable: bool, pk: bool) -> Column {
        Column {
            name: name.to_owned(),
            pg_type: ty.to_owned(),
            udt_name: udt.to_owned(),
            nullable,
            is_primary_key: pk,
        }
    }

    #[test]
    fn pg_to_rust_scalars() {
        assert_eq!(pg_to_rust("integer", "int4"), Some("i32"));
        assert_eq!(pg_to_rust("bigint", "int8"), Some("i64"));
        assert_eq!(pg_to_rust("smallint", "int2"), Some("i16"));
        assert_eq!(pg_to_rust("text", "text"), Some("String"));
        assert_eq!(pg_to_rust("character varying", "varchar"), Some("String"));
        assert_eq!(pg_to_rust("boolean", "bool"), Some("bool"));
        assert_eq!(pg_to_rust("timestamp with time zone", "timestamptz"), Some("chrono::DateTime<chrono::Utc>"));
        assert_eq!(pg_to_rust("uuid", "uuid"), Some("uuid::Uuid"));
        assert_eq!(pg_to_rust("jsonb", "jsonb"), Some("serde_json::Value"));
    }

    #[test]
    fn pg_to_rust_falls_back_to_udt() {
        // When data_type is generic, udt_name may carry the real info.
        assert_eq!(pg_to_rust("USER-DEFINED", "int4"), Some("i32"));
    }

    #[test]
    fn pg_to_rust_arrays() {
        assert_eq!(pg_to_rust("ARRAY", "_text"), Some("Vec<String>"));
        assert_eq!(pg_to_rust("ARRAY", "_int4"), Some("Vec<i32>"));
    }

    #[test]
    fn pg_to_rust_unknown_returns_none() {
        assert_eq!(pg_to_rust("box", "box"), None);
    }

    #[test]
    fn nullable_wraps_in_option() {
        let c = col("name", "text", "text", true, false);
        assert_eq!(rust_type_for(&c), "Option<String>");
    }

    #[test]
    fn non_nullable_no_option() {
        let c = col("id", "bigint", "int8", false, true);
        assert_eq!(rust_type_for(&c), "i64");
    }

    #[test]
    fn unmapped_type_emits_todo() {
        let c = col("polygon_col", "polygon", "polygon", false, false);
        let s = rust_type_for(&c);
        assert!(s.contains("serde_json::Value"));
        assert!(s.contains("TODO"));
        assert!(s.contains("polygon"));
    }

    #[test]
    fn pascal_case_basic() {
        assert_eq!(to_pascal_case("users"), "Users");
        assert_eq!(to_pascal_case("audit_log_entry"), "AuditLogEntry");
        assert_eq!(to_pascal_case("api-keys"), "ApiKeys");
        assert_eq!(to_pascal_case("HTTPResponse"), "HTTPResponse");
    }

    #[test]
    fn render_basic_table() {
        let t = Table {
            schema: "public".to_owned(),
            name: "users".to_owned(),
            columns: vec![
                col("id", "bigint", "int8", false, true),
                col("email", "text", "text", false, false),
                col("nickname", "text", "text", true, false),
            ],
        };
        let s = render_rust(&t);
        // The exact whitespace doesn't matter; the structure does.
        assert!(s.contains("pub struct Users {"));
        assert!(s.contains("// primary key"));
        assert!(s.contains("pub id: i64,"));
        assert!(s.contains("pub email: String,"));
        assert!(s.contains("pub nickname: Option<String>,"));
        assert!(s.contains("impl ormx::Model for Users"));
        assert!(s.contains("fn table_name() -> &'static str { \"users\" }"));
    }

    #[test]
    fn reserved_keyword_gets_raw_prefix() {
        let t = Table {
            schema: "public".to_owned(),
            name: "audit".to_owned(),
            columns: vec![
                col("id", "bigint", "int8", false, true),
                col("type", "text", "text", false, false), // `type` is reserved
            ],
        };
        let s = render_rust(&t);
        assert!(s.contains("pub r#type: String,"));
    }

    #[test]
    fn render_all_emits_module_header() {
        let t = Table {
            schema: "public".to_owned(),
            name: "t".to_owned(),
            columns: vec![col("id", "integer", "int4", false, true)],
        };
        let s = render_rust_all(&[t]);
        assert!(s.contains("Generated by `ormx::codegen`"));
        assert!(s.contains("#![allow(dead_code"));
        assert!(s.contains("pub struct T {"));
    }

    // ---- Phase 2 renderers ----

    fn sample_users_table() -> Table {
        Table {
            schema: "public".to_owned(),
            name: "users".to_owned(),
            columns: vec![
                col("id", "bigint", "int8", false, true),
                col("email", "text", "text", false, false),
                col("nickname", "text", "text", true, false),
                col("age", "integer", "int4", false, false),
                col("active", "boolean", "bool", false, false),
            ],
        }
    }

    #[test]
    fn render_python_basic() {
        let t = sample_users_table();
        let s = render_python(&t);
        assert!(s.contains("class Users(Model):"), "got: {s}");
        assert!(s.contains("table_name = \"users\""));
        assert!(s.contains("id: int = PrimaryKey()"));
        assert!(s.contains("email: str"));
        assert!(s.contains("nickname: Optional[str]"));
        assert!(s.contains("age: int"));
        assert!(s.contains("active: bool"));
    }

    #[test]
    fn render_node_basic() {
        let t = sample_users_table();
        let s = render_node(&t);
        assert!(s.contains("export class Users extends Model"), "got: {s}");
        assert!(s.contains("static tableName = \"users\""));
        assert!(s.contains("// primary key"));
        assert!(s.contains("declare id: bigint;"));
        assert!(s.contains("declare email: string;"));
        assert!(s.contains("declare nickname: string | null;"));
        assert!(s.contains("declare age: number;"));
        assert!(s.contains("declare active: boolean;"));
    }

    #[test]
    fn render_go_basic() {
        let t = sample_users_table();
        let s = render_go(&t);
        assert!(s.contains("type Users struct {"), "got: {s}");
        assert!(s.contains("Id int64 `db:\"id\"`"));
        assert!(s.contains("Email string `db:\"email\"`"));
        assert!(s.contains("Nickname *string `db:\"nickname\"`"));
        assert!(s.contains("Age int32 `db:\"age\"`"));
        assert!(s.contains("Active bool `db:\"active\"`"));
        assert!(s.contains("func (Users) TableName() string { return \"users\" }"));
    }

    #[test]
    fn render_ruby_basic() {
        let t = sample_users_table();
        let s = render_ruby(&t);
        assert!(s.contains("class Users < Ormx::Model"), "got: {s}");
        assert!(s.contains("table_name \"users\""));
        assert!(s.contains("attributes :id, :email, :nickname, :age, :active"));
        assert!(s.contains("# primary key: id"));
    }

    #[test]
    fn render_java_basic() {
        let t = sample_users_table();
        let s = render_java(&t);
        assert!(s.contains("public record Users("), "got: {s}");
        // NOT NULL columns get primitive types
        assert!(s.contains("long id"));
        assert!(s.contains("int age"));
        assert!(s.contains("boolean active"));
        assert!(s.contains("String email"));
        // Nullable column gets the boxed type
        assert!(s.contains("String nickname"));
        // primary-key comment up top
        assert!(s.contains("// primary key: id"));
    }

    #[test]
    fn java_camel_case_basic() {
        assert_eq!(to_java_camel_case("user_id"), "userId");
        assert_eq!(to_java_camel_case("audit_log_entry"), "auditLogEntry");
        assert_eq!(to_java_camel_case("id"), "id");
        assert_eq!(to_java_camel_case("FirstName"), "firstName");
    }

    #[test]
    fn java_composite_pk_renders_both() {
        let t = Table {
            schema: "public".to_owned(),
            name: "membership".to_owned(),
            columns: vec![
                col("org_id", "integer", "int4", false, true),
                col("user_id", "integer", "int4", false, true),
                col("role", "text", "text", false, false),
            ],
        };
        let s = render_java(&t);
        assert!(s.contains("// primary key: org_id, user_id"));
        assert!(s.contains("int orgId"));
        assert!(s.contains("int userId"));
    }

    #[test]
    fn python_unmapped_falls_through_to_any() {
        let t = Table {
            schema: "public".to_owned(),
            name: "x".to_owned(),
            columns: vec![col("p", "polygon", "polygon", false, false)],
        };
        let s = render_python(&t);
        // Unknown pg type → `Any` in Python.
        assert!(s.contains("p: Any"));
    }

    #[test]
    fn go_jsonb_uses_raw_message() {
        let t = Table {
            schema: "public".to_owned(),
            name: "x".to_owned(),
            columns: vec![col("data", "jsonb", "jsonb", false, false)],
        };
        let s = render_go(&t);
        assert!(s.contains("Data json.RawMessage"));
    }
}
