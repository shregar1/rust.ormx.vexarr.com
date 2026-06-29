//! SQL dialect — drives placeholder syntax and a couple of feature-detection
//! checks. Variants for wire-compatible variants (CockroachDB, Neon, TiDB,
//! PlanetScale, …) route to the same render rules as their base protocol; they
//! exist so callers can name what they're connecting to without surprises.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    // -- Postgres family ($1, $2, …; supports RETURNING) ----------------
    Postgres,
    CockroachDb,
    Yugabyte,
    Neon,

    // -- MySQL family (?, ?, …; no RETURNING) ---------------------------
    MySql,
    MariaDb,
    TiDb,
    PlanetScale,

    // -- SQLite family (?, ?, …; supports RETURNING since 3.35) --------
    Sqlite,
    LibSql,

    // -- Oracle (:1, :2, …; no RETURNING via this DSL) ------------------
    // SQL-gen only — there is no first-class sqlx driver for Oracle, so
    // there is no Executor impl. Use the raw escape hatch + your own driver.
    Oracle,

    // -- SQL Server (@p1, @p2, …; uses OUTPUT, not RETURNING) -----------
    // SQL-gen only. sqlx dropped MSSQL in 0.8; connect via tiberius or
    // a community crate and feed our rendered SQL through it.
    MsSql,

    // -- DB2 (?, ?, …; FINAL TABLE wraps DML for "returning") -----------
    // SQL-gen only. No sqlx driver; use ibm_db / odbc-api on top.
    Db2,
}

impl Dialect {
    /// Render the placeholder for the Nth parameter (1-indexed).
    pub fn placeholder(self, n: usize) -> String {
        match self.family() {
            Family::Postgres => format!("${}", n),
            Family::MySql | Family::Sqlite | Family::Db2 => "?".to_owned(),
            Family::Oracle => format!(":{}", n),
            Family::MsSql => format!("@p{}", n),
        }
    }

    /// Whether `RETURNING *` is valid on INSERT/UPDATE/DELETE.
    pub fn supports_returning(self) -> bool {
        matches!(self.family(), Family::Postgres | Family::Sqlite)
    }

    /// Underlying wire-protocol family.
    pub fn family(self) -> Family {
        match self {
            Dialect::Postgres | Dialect::CockroachDb | Dialect::Yugabyte | Dialect::Neon => {
                Family::Postgres
            }
            Dialect::MySql | Dialect::MariaDb | Dialect::TiDb | Dialect::PlanetScale => {
                Family::MySql
            }
            Dialect::Sqlite | Dialect::LibSql => Family::Sqlite,
            Dialect::Oracle => Family::Oracle,
            Dialect::MsSql => Family::MsSql,
            Dialect::Db2 => Family::Db2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Postgres,
    MySql,
    Sqlite,
    Oracle,
    MsSql,
    Db2,
}
