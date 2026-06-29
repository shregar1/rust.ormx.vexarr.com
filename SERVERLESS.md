# Serverless Postgres compatibility

ormx talks the standard Postgres wire protocol. Every managed/serverless
Postgres provider that exposes the same wire works transparently — you
point your DSN at them and the same `Model::find / query() / createMany`
calls flow through.

## Tested compatible

| Provider | DSN format | Notes |
|---|---|---|
| **AWS RDS / Aurora Postgres** | `postgres://user:pw@host:5432/db?sslmode=require` | TLS required in prod. |
| **Google Cloud SQL (Postgres)** | `postgres://user:pw@host:5432/db?sslmode=require` | Use Cloud SQL Auth Proxy for private IP. |
| **Azure Database for PostgreSQL** | `postgres://user@server:pw@server.postgres.database.azure.com/db?sslmode=require` | User is `username@servername`. |
| **Neon** | `postgres://user:pw@ep-xxx.region.aws.neon.tech/db?sslmode=require` | Connection pooling: append `-pooler` to endpoint. |
| **Supabase** | `postgres://postgres:pw@db.xxx.supabase.co:5432/postgres?sslmode=require` | Connection pooler at port 6543 (PgBouncer; some features off). |
| **CockroachDB Serverless** | `postgresql://user:pw@host.cockroachlabs.cloud:26257/db?sslmode=verify-full` | Set `Dialect::CockroachDb` in `ormx-rust-client` for correct upsert handling. |
| **Yugabyte** | `postgres://user:pw@host:5433/db?sslmode=require` | `Dialect::Yugabyte`. |
| **Render Postgres** | `postgres://user:pw@host/db?sslmode=require` | Internal + external URLs differ; use external for non-Render apps. |
| **Railway Postgres** | `postgres://postgres:pw@host:port/railway?sslmode=require` | |
| **Xata** | `postgresql://user:pw@us-east-1.sql.xata.sh:5432/db:branch?sslmode=require` | Branch is part of the path. |
| **Fly.io Postgres** | `postgres://postgres:pw@<app>.internal:5432/db` | Fly's internal DNS; no SSL on private net. |

## Tested compatible (MySQL family — `--features mysql`)

| Provider | Dialect |
|---|---|
| **PlanetScale** | `Dialect::PlanetScale` |
| **TiDB Cloud** | `Dialect::TiDb` |
| **AWS Aurora MySQL** | `Dialect::MySql` |
| **MariaDB Cloud (SkySQL)** | `Dialect::MariaDb` |

## The `Dialect` matters because

Each provider's "Postgres" or "MySQL" wire-compatible mode has subtle
differences from upstream:

- **CockroachDB**: doesn't support `ON CONFLICT DO UPDATE` exactly like
  Postgres; we emit `UPSERT` instead when `Dialect::CockroachDb`.
- **Yugabyte**: returns `xmax = 0` differently on conflicts.
- **PlanetScale**: doesn't allow foreign keys; query builder emits no
  FK assumptions.
- **TiDB**: doesn't support `LAST_INSERT_ID()` semantics like MySQL —
  we use `RETURNING` if `Dialect::TiDb` (TiDB added it in 7.1).

When in doubt, pick the most specific dialect that matches your wire.
Wrong dialect → SQL that "works" most of the time and breaks on the
one feature your provider quirks.

## SSL/TLS notes

All commercial managed Postgres providers require `sslmode=require` in
the DSN. `ormx-rust-client` uses sqlx underneath which handles this
transparently; just include the query param.

For self-signed certs (private CA), use `sslmode=verify-full` and pass
the CA cert path via `sslrootcert=/path/to/ca.pem`.

## What doesn't work transparently

- **DataAPI-style HTTP gateways** (AWS RDS Data API, Neon HTTP driver,
  Supabase REST). These are HTTP shapes, not Postgres wire — ormx
  can't talk to them directly. Use the provider's native driver and
  feed raw SQL via `ormx::raw`.
- **Read replicas with automatic routing**. ormx points at one pool
  at a time; route reads to a replica DSN by hand or use a connection
  pooler that does this for you (PgBouncer, RDS Proxy).
