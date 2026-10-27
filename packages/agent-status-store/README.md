`agent-status-store` stores Agent Status telemetry and dashboard state for ingesters, read-only servers, and offline debugging tools. It lets callers commit event history, usage observations, agent state, and delivery cursors together.

# What It Does

- **Durable state**: Stores environments, runs, agents, telemetry, observer cursors, and explicit skill, command, and MCP usage.
- **Historical upgrades**: Migrates schema versions 1–10 and backfills usage from retained telemetry.
- **Dashboard queries**: Reads filtered agents, projects, attention counts, paginated events, subagents, and usage summaries.
- **Read-only access**: Opens dashboard databases without creating or upgrading them.

# How It Works

1. Open a writer or an existing read-only database.
2. Begin a transaction for related writes or a consistent aggregate snapshot.
3. Save state and query results using the shared Agent Status data types.
4. Commit successful writes before acknowledging their delivery.

# How it Really Works

1. Writer calls create missing parent directories and upgrade the database before returning. File databases use a write-ahead log, normal synchronization, foreign keys, and a five-second lock timeout.
2. Calls block the calling thread. Callers own connections and transactions and arrange their own asynchronous scheduling and synchronization.
3. Each schema version commits atomically. Unsupported versions, foreign-key violations, invalid stored JSON, and database failures return typed errors. A run counts as running only while it exists and has no recorded end time.
4. Pruning telemetry leaves durable usage intact. Deleting an agent cascades to its usage. Nothing reads environment configuration or sends network traffic.

# Installation

This library is available in the local migration checkout:

```sh
cargo add agent-status-store --path ../agent-status/packages/agent-status-store
```

# Quick Start

```rust
use agent_status_store::{open_writer, read};

fn main() -> agent_status_store::Result<()> {
    let mut connection = open_writer(":memory:")?;
    let snapshot = connection.transaction()?;
    let status = read::status(&snapshot, 1_000)?;
    assert_eq!(status.active_agent_count, 0);
    assert_eq!(status.requires_attention_count, 0);
    Ok(())
}
```

# API

| Entry point | Result |
| :--- | :--- |
| `open_writer(path)` | Writable connection with migrations applied |
| `open_reader(path)` | Existing database opened read-only |
| `schema::migrate(&mut connection)` | Historical upgrades on an owned connection |
| `write::{save_agent, record_event, save_cursor}` | State, telemetry/usage, and cursor writes inside a caller transaction |
| `write::{upsert_environment, heartbeat, dropped_frames}` | Environment identity and delivery health updates |
| `write::{set_visibility, dismiss_completion, end_current_run}` | Explicit state mutations |
| `write::prune` | Event, agent, and run retention inside a transaction |
| `read::{agent, agents, projects, environments}` | Typed dashboard state and filtering |
| `read::{status, last_sequence}` | Snapshot fleet totals and persisted delivery progress |
| `read::{events, distinct_subagents, usage}` | Paginated history and durable usage summaries |

Event pagination is descending by event ID, uses an exclusive `before` cursor, defaults to 100 rows, and caps requests at 1,000 rows. Returned frames carry event IDs in `seq` and current agent metadata. Empty subagent filters select all events. Missing agents return `Error::NotFound`; event queries with no matching rows return an empty vector.

# License

[MIT](LICENSE), copyright 2026 Alex Gorbatchev.
