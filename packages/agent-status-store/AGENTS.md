---
created_on: 2026-10-14 10:49
last_modified: 2026-10-14 10:49
status: current
---

# Agent Status storage

Shared SQLite schema, writer operations, and dashboard queries for Agent Status services and offline debugging.

## Commands

- `just check`: package formatting, strict Clippy, behavioral tests, and line coverage.
- `just build`: optimized library build.
- `just test`: real SQLite migration, transaction, and query tests.

## Contracts

- Keep callers on native `rusqlite::Connection` and `Transaction`; every mutation receives the caller's transaction. Event, usage, derived state, and observer cursor belong in one transaction before acknowledgement.
- Readers open databases read-only and never migrate or create them. Aggregate status queries receive a transaction to preserve one snapshot.
- Preserve historical schema versions 1–10 and usage backfill. Foreign keys must be disabled before the version-six rebuild transaction and restored after failure as well as success. Validate foreign keys before committing the rebuild.
- Lifecycle is RUNNING only when a referenced run exists and has no end time. Use this rule consistently in individual, filtered, grouped, and aggregate queries.
- Keep usage independent of telemetry retention; deduplicate native observation identity within each agent.
- Propagate corrupt stored JSON as a typed error rather than silently returning fabricated empty state.

## Boundaries

- Read applicable skills before changing source or documentation. Runtime changes require corresponding behavioral test changes and at least 90% measured line coverage; `scripts/` is excluded.
- Prove meaningful changes by temporarily disabling them, recording the expected failing test, restoring the implementation, and rerunning checks.
- Use `.tmp/` for evidence and temporary files. Keep source checkouts and sibling projects untouched; scope formatters to this package.
- Persist standing rules in place; keep progress and handoffs outside this file.
- Never publish releases, tags, packages, or production deployments without explicit authorization.
