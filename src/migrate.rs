//! Schema migrations — Phase 1 (Postgres only).
//!
//! Convention: a directory of plain SQL files, named
//! `<version>__<name>.up.sql` and an optional matching `*.down.sql`.
//! Versions are opaque strings that sort lexicographically — pick
//! anything monotonic, e.g. a timestamp `20260519143000` or a zero-
//! padded sequence `0001`.
//!
//! Applied migrations get recorded in `_ormx_migrations(version,
//! name, checksum, applied_at)`. The checksum is SHA-256 of the UP SQL,
//! so editing an already-applied migration on disk surfaces as drift
//! in `status()` instead of being silently re-applied.
//!
//! Each migration runs in its own `BEGIN` / `COMMIT`. If a migration's
//! SQL fails mid-stream the surrounding transaction rolls back and
//! nothing gets recorded in `_ormx_migrations` — re-running picks up
//! exactly where it left off.
//!
//! ```ignore
//! use ormx::migrate;
//! use sqlx::postgres::PgPoolOptions;
//!
//! let pool = PgPoolOptions::new().connect("postgres://…").await?;
//! let migrations = migrate::load_dir("./migrations".as_ref())?;
//!
//! // Apply everything pending.
//! let applied = migrate::up(&pool, &migrations, None).await?;
//! println!("applied: {:?}", applied);
//!
//! // Roll back one step.
//! migrate::down(&pool, &migrations, None).await?;
//!
//! // Where are we now?
//! let plan = migrate::status(&pool, &migrations).await?;
//! ```

use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::{Error, Result};

const MIGRATIONS_TABLE: &str = "_ormx_migrations";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// One migration loaded from disk. Equality is by `version` only —
/// the rest is content that may legitimately differ between revisions
/// of the same migration (and the checksum field is what we use to
/// surface that drift).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    pub version: String,
    pub name: String,
    pub up_sql: String,
    pub down_sql: Option<String>,
    pub checksum: String,
}

/// A row from `_ormx_migrations` — what's recorded as already applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub version: String,
    pub name: String,
    pub checksum: String,
}

/// State snapshot: what's applied, what's pending, and any drift
/// (versions where the on-disk checksum disagrees with the recorded
/// one — typically means someone edited a committed migration).
#[derive(Debug, Clone)]
pub struct Plan {
    pub applied: Vec<Applied>,
    pub pending: Vec<Migration>,
    pub drift: Vec<DriftedMigration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftedMigration {
    pub version: String,
    pub recorded_checksum: String,
    pub disk_checksum: String,
}

// ---------------------------------------------------------------------------
// Loading from disk
// ---------------------------------------------------------------------------

/// Walk `dir`, pair up `.up.sql` / `.down.sql` files, hash the UP
/// content, return migrations sorted by `version`.
///
/// Files that don't match the convention are quietly ignored, so the
/// directory can also hold a README or a `.gitkeep`.
pub fn load_dir(dir: &Path) -> Result<Vec<Migration>> {
    if !dir.is_dir() {
        return Err(Error::Migration(format!(
            "migrations directory not found: {}",
            dir.display()
        )));
    }

    use std::collections::BTreeMap;
    // (version, name) → (up_sql, down_sql)
    let mut acc: BTreeMap<(String, String), (Option<String>, Option<String>)> = BTreeMap::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let (version, name, kind) = match parse_filename(fname) {
            Some(parts) => parts,
            None => continue,
        };
        let body = fs::read_to_string(&path)?;
        let slot = acc.entry((version, name)).or_insert((None, None));
        match kind {
            FileKind::Up => slot.0 = Some(body),
            FileKind::Down => slot.1 = Some(body),
        }
    }

    let mut out = Vec::with_capacity(acc.len());
    for ((version, name), (up, down)) in acc {
        let up_sql = up.ok_or_else(|| {
            Error::Migration(format!(
                "{version}__{name}.down.sql exists but no matching .up.sql"
            ))
        })?;
        let checksum = sha256_hex(&up_sql);
        out.push(Migration {
            version,
            name,
            up_sql,
            down_sql: down,
            checksum,
        });
    }
    // BTreeMap iterates in key order already; just be explicit.
    out.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(out)
}

enum FileKind {
    Up,
    Down,
}

/// Parse `<version>__<name>.<kind>.sql`. Returns None for any other shape.
fn parse_filename(s: &str) -> Option<(String, String, FileKind)> {
    let stem = s.strip_suffix(".sql")?;
    let (head, kind) = if let Some(h) = stem.strip_suffix(".up") {
        (h, FileKind::Up)
    } else if let Some(h) = stem.strip_suffix(".down") {
        (h, FileKind::Down)
    } else {
        return None;
    };
    let (version, name) = head.split_once("__")?;
    if version.is_empty() || name.is_empty() {
        return None;
    }
    Some((version.to_owned(), name.to_owned(), kind))
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{:02x}", b);
    }
    out
}

// ---------------------------------------------------------------------------
// DB plumbing
// ---------------------------------------------------------------------------

async fn ensure_table(pool: &PgPool) -> Result<()> {
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {MIGRATIONS_TABLE} (
            version    TEXT PRIMARY KEY,
            name       TEXT NOT NULL,
            checksum   TEXT NOT NULL,
            applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    );
    sqlx::query(&sql).execute(pool).await?;
    Ok(())
}

async fn read_applied(pool: &PgPool) -> Result<Vec<Applied>> {
    let sql = format!(
        "SELECT version, name, checksum FROM {MIGRATIONS_TABLE} ORDER BY version ASC"
    );
    let rows: Vec<(String, String, String)> = sqlx::query_as(&sql).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(version, name, checksum)| Applied { version, name, checksum })
        .collect())
}

// ---------------------------------------------------------------------------
// Public surface: status / up / down
// ---------------------------------------------------------------------------

/// Reconcile on-disk migrations with `_ormx_migrations`. Doesn't mutate.
pub async fn status(pool: &PgPool, migrations: &[Migration]) -> Result<Plan> {
    ensure_table(pool).await?;
    let applied = read_applied(pool).await?;

    use std::collections::HashMap;
    let applied_idx: HashMap<&str, &Applied> = applied.iter().map(|a| (a.version.as_str(), a)).collect();

    let mut pending = Vec::new();
    let mut drift = Vec::new();
    for m in migrations {
        match applied_idx.get(m.version.as_str()) {
            None => pending.push(m.clone()),
            Some(a) if a.checksum != m.checksum => drift.push(DriftedMigration {
                version: m.version.clone(),
                recorded_checksum: a.checksum.clone(),
                disk_checksum: m.checksum.clone(),
            }),
            Some(_) => { /* applied and clean */ }
        }
    }
    Ok(Plan { applied, pending, drift })
}

/// Apply pending migrations up to (and including) `target`, or all
/// pending if `target` is `None`. Returns the versions that ran in
/// this call. Refuses to proceed if drift is detected.
pub async fn up(
    pool: &PgPool,
    migrations: &[Migration],
    target: Option<&str>,
) -> Result<Vec<String>> {
    let plan = status(pool, migrations).await?;
    if !plan.drift.is_empty() {
        let versions: Vec<&str> = plan.drift.iter().map(|d| d.version.as_str()).collect();
        return Err(Error::Migration(format!(
            "drift detected — applied migrations differ from disk: {versions:?}. \
             Restore the on-disk file or roll back & re-apply."
        )));
    }
    // Validate the target exists (if given).
    if let Some(t) = target {
        if !migrations.iter().any(|m| m.version == t) {
            return Err(Error::Migration(format!("unknown target version {t}")));
        }
    }

    let mut applied = Vec::new();
    for m in &plan.pending {
        if let Some(t) = target {
            if m.version.as_str() > t {
                break;
            }
        }
        run_one_up(pool, m).await?;
        applied.push(m.version.clone());
    }
    Ok(applied)
}

async fn run_one_up(pool: &PgPool, m: &Migration) -> Result<()> {
    let mut tx = pool.begin().await?;
    // The migration body may contain multiple statements separated by `;`.
    // sqlx's `query().execute(&mut *tx)` runs them as a simple-query
    // batch when there are no parameters — exactly what we want.
    sqlx::query(&m.up_sql).execute(&mut *tx).await.map_err(|e| {
        Error::Migration(format!("UP {version}__{name} failed: {e}", version = m.version, name = m.name))
    })?;
    let insert = format!(
        "INSERT INTO {MIGRATIONS_TABLE} (version, name, checksum) VALUES ($1, $2, $3)"
    );
    sqlx::query(&insert)
        .bind(&m.version)
        .bind(&m.name)
        .bind(&m.checksum)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Roll back applied migrations down to (but **not** including) `target`.
/// If `target` is `None`, rolls back exactly one — the most recently
/// applied. Returns the versions that ran in this call (in the order
/// they were rolled back: newest first).
pub async fn down(
    pool: &PgPool,
    migrations: &[Migration],
    target: Option<&str>,
) -> Result<Vec<String>> {
    ensure_table(pool).await?;
    let applied = read_applied(pool).await?;
    if applied.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(t) = target {
        // target must exist (either applied or on-disk), otherwise we'd
        // happily blow away everything.
        let exists = applied.iter().any(|a| a.version == t)
            || migrations.iter().any(|m| m.version == t);
        if !exists {
            return Err(Error::Migration(format!("unknown target version {t}")));
        }
    }

    use std::collections::HashMap;
    let by_version: HashMap<&str, &Migration> =
        migrations.iter().map(|m| (m.version.as_str(), m)).collect();

    let mut rolled = Vec::new();
    for a in applied.iter().rev() {
        if let Some(t) = target {
            if a.version.as_str() <= t {
                break;
            }
        }
        let m = by_version.get(a.version.as_str()).ok_or_else(|| {
            Error::Migration(format!(
                "{} is applied but missing from the migrations directory — \
                 cannot roll back without its .down.sql",
                a.version
            ))
        })?;
        let down = m.down_sql.as_deref().ok_or_else(|| {
            Error::Migration(format!(
                "{}__{} has no .down.sql; cannot reverse",
                m.version, m.name
            ))
        })?;
        run_one_down(pool, m, down).await?;
        rolled.push(a.version.clone());
        // No-target → roll back exactly one and stop.
        if target.is_none() {
            break;
        }
    }
    Ok(rolled)
}

async fn run_one_down(pool: &PgPool, m: &Migration, down: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(down).execute(&mut *tx).await.map_err(|e| {
        Error::Migration(format!(
            "DOWN {version}__{name} failed: {e}",
            version = m.version,
            name = m.name
        ))
    })?;
    let del = format!("DELETE FROM {MIGRATIONS_TABLE} WHERE version = $1");
    sqlx::query(&del).bind(&m.version).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Unit tests — pure parsing logic only. End-to-end DB tests live in tests/.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_up_filename() {
        let (v, n, k) = parse_filename("20260519143000__create_users.up.sql").unwrap();
        assert_eq!(v, "20260519143000");
        assert_eq!(n, "create_users");
        assert!(matches!(k, FileKind::Up));
    }

    #[test]
    fn parse_down_filename() {
        let (v, n, k) = parse_filename("0001__init.down.sql").unwrap();
        assert_eq!(v, "0001");
        assert_eq!(n, "init");
        assert!(matches!(k, FileKind::Down));
    }

    #[test]
    fn unknown_shapes_ignored() {
        assert!(parse_filename("README.md").is_none());
        assert!(parse_filename("0001_init.up.sql").is_none()); // single underscore
        assert!(parse_filename("__missing_version.up.sql").is_none());
        assert!(parse_filename("0001__.up.sql").is_none()); // missing name
        assert!(parse_filename("0001__init.sql").is_none()); // missing kind
    }

    #[test]
    fn checksum_is_stable() {
        assert_eq!(
            sha256_hex("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn checksum_differs_on_whitespace() {
        // We deliberately don't normalize — any byte change is drift.
        assert_ne!(sha256_hex("SELECT 1"), sha256_hex("SELECT 1\n"));
    }
}
