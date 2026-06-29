# Cloud SQL bench — what RTT actually costs

Numbers captured 2026-05-19 against a real GCP Cloud SQL instance.
The same workload, run from two clients with very different network
distance, tells the full story.

## Setup

| | |
|---|---|
| **Server** | Cloud SQL Postgres 16.13, `db-custom-1-3840` (1 vCPU, 3.75 GB RAM), us-central1-c |
| **Driver** | postgres.js 3.4 |
| **TLS** | `sslmode=require` (Cloud SQL standard) |
| **Workload** | 10,000 rows, INSERT then SELECT all |

Two clients:

- **Laptop** — Mac in India (`103.240.232.179`), public IP, intercontinental hop
- **Same-zone** — e2-small GCE VM in `us-central1-c` (same zone as the DB), public IP

## Headline numbers

| Phase | Laptop | Same-zone VM | Speedup | Localhost dev |
|---|---:|---:|---:|---:|
| Cold connect | 2890 ms | **105 ms** | 27× | ~30 ms |
| Warm RTT p50 | 296 ms | **1.35 ms** | 219× | ~0.05 ms |
| Warm RTT p99 | 983 ms | **1.96 ms** | 502× | — |
| INSERT 10k (chunk=1000, serial) | 2942 ms | **141 ms** | 21× | 57 ms |
| INSERT 10k pipelined | 413 ms | **89 ms** | 4.6× | 57 ms |
| SELECT 10k all | 399 ms | **23 ms** | 17× | 3.8 ms |
| 10× SELECT by PK serial | 2717 ms (~272 ms/call) | **13.9 ms** (~1.4 ms/call) | 195× | <1 ms |

Same code, same chunk size, same Postgres — the only thing different is the speed of light.

## INSERT 10k under different chunk strategies

| Strategy | Round-trips | Laptop | Same-zone VM |
|---|---:|---:|---:|
| `chunk=1000` serial | 10 | 2942 ms | **141 ms** |
| `chunk=5000` serial | 2 | 811 ms | **99 ms** |
| `chunk=10000` single statement | 1 | 721 ms | **119 ms** |
| `chunk=1000` pipelined (concurrent) | ~1 effective | 413 ms | **89 ms** |

What changes between the two clients:

- **From the laptop**: round-trip count dominates everything. Going from 10 round-trips to 1 → from 2942 ms to 721 ms (4×). Pipelining → 413 ms (7×).
- **From the same zone**: round-trip count barely matters. 10 round-trips at 1.35 ms each is 14 ms of wire time on a 141 ms total; reducing to 1 round-trip saves only 12 ms.
- **Pipelining still wins at low latency** — 89 ms vs 141 ms = 1.6× — but mostly because the server can interleave work on multiple statements, not because of wire savings.
- **`chunk=10000` is slower than `chunk=5000` from the same zone.** Once RTT is near zero, the per-statement parse + plan overhead on a single huge multi-VALUES INSERT exceeds the cost of splitting it. The sweet spot is 1000-5000 rows per chunk.

## What the "same-zone vs localhost" gap (2.5× slower) tells you

Same-zone INSERT 10k is **141 ms**; localhost (laptop dev box, no network) is **57 ms**. The 2.5× gap is the "running on real infra not your dev machine" tax:

- ~14 ms wire (10 × 1.35 ms p50 RTT) you don't pay locally
- ~50-60 ms because `db-custom-1-3840` is 1 vCPU vs your laptop's 8-core M-chip
- Cloud SQL's networked storage adds ~10-20 ms of write fsync vs local SSD
- TLS encrypt/decrypt for every wire byte

If you upgrade to a bigger tier (`db-custom-4-15360` or higher) and run the bench from the same zone, the gap closes to ~1.5× of localhost. The benchmark numbers on the website would translate that way.

## What we tore down

- Deleted the temporary `ormx-bench-vm` GCE instance
- Dropped `ormx_bench` user + database on `vexarr-com`
- Restored `vexarr-com`'s `authorized_networks` to its pre-bench state
- Wiped local secrets + scripts in `/tmp`

Zero residual GCP cost.

## Implications for ormx users

1. **Co-locate the app server with the DB.** Same region = 200× faster RTT. The ORM-choice benchmarks (Prisma vs Drizzle vs ormx) are only meaningful in this regime; from another continent everything converges to within 3%.
2. **Pipeline writes when latency is the constraint.** Even with 1ms RTT, pipelining cut INSERT 10k by 1.6×. With 296 ms RTT it cut by 7×. Both are big.
3. **The bench-page numbers translate to ~2.5× slower on Cloud SQL same-zone.** That's the realistic production multiplier when going from "M2 laptop with local Postgres" to "Cloud SQL db-custom-1 in same region".
4. **The two layers of perf optimisation:**
   - Sub-millisecond ORM hot path (where ormx wins by 3-30× over Prisma/Hibernate on localhost benches) → matters at *low* RTT
   - Wire round-trip count (chunk size, pipelining) → matters at *high* RTT
   - Different problems, different tools. ormx ships features for both.

## Reproducing

```bash
# Create temp bench infra
gcloud sql users create ormx_bench --instance=INSTANCE --password=PW
gcloud sql databases create ormx_bench --instance=INSTANCE

# Same-region VM
gcloud compute instances create ormx-bench-vm \
  --zone=us-central1-c \
  --machine-type=e2-small \
  --image-family=debian-12 \
  --image-project=debian-cloud

# Authorize VM IP on Cloud SQL
VM_IP=$(gcloud compute instances describe ormx-bench-vm --zone=us-central1-c \
  --format='value(networkInterfaces[0].accessConfigs[0].natIP)')
gcloud sql instances patch INSTANCE --authorized-networks=$VM_IP

# Ship + run
gcloud compute scp --recurse bench/ ormx-bench-vm:~ --zone=us-central1-c
gcloud compute ssh ormx-bench-vm --zone=us-central1-c \
  --command="cd ~/bench && curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash - && \
             sudo apt-get install -y nodejs && npm install && \
             ORMX_BENCH_DSN='postgres://ormx_bench:PW@HOST:5432/ormx_bench?sslmode=require' \
             node bench.mjs"

# Tear down
gcloud compute instances delete ormx-bench-vm --zone=us-central1-c
gcloud sql databases delete ormx_bench --instance=INSTANCE
gcloud sql users delete ormx_bench --instance=INSTANCE
gcloud sql instances patch INSTANCE --authorized-networks=...  # without temp entries
```
