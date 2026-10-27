Agent Status collects coding-agent telemetry, stores activity and usage, serves dashboard queries, and provides offline diagnostics. This Rust workspace preserves the package grouping of [the original Agent Status repository](https://github.com/alexgorbatchev/agent-status).

# What It Does

- **Observe sessions:** Follow Claude Code, Pi, Codex, and OpenCode activity and deliver acknowledged telemetry.
- **Ingest activity:** Commit telemetry, current state, usage, and delivery checkpoints together.
- **Serve queries:** Provide read-only database queries and live streams, forwarding explicit mutations to the ingester.
- **Diagnose locally:** Replay transcripts, inspect stored records, and audit native session discovery.

# How It Works

1. Run an ingester to own the database and local notification socket.
2. Run observers on machines hosting coding agents.
3. Run the server against the ingester's database and socket for dashboard clients.
4. Use Agent Debug independently for local replay and inspection.

# How it Really Works

One Git repository contains six Cargo packages and one workspace lockfile. The ingester owns persistent writes; the server opens its input database read-only. Observers retain unacknowledged deliveries across reconnects. Debug normally replays in memory and writes only when explicitly exporting a database. Provider summarization is configurable and can incur charges.

| Package | Role | Documentation |
| :--- | :--- | :--- |
| `agent-common` | Shared telemetry, usage, models, logging | [API and configuration](packages/agent-common/README.md) |
| `agent-status-store` | Transactional SQLite schema and queries | [Storage API](packages/agent-status-store/README.md) |
| `agent-observer` | Local discovery and acknowledged delivery | [Observer guide](packages/agent-observer/README.md) |
| `agent-ingester` | Durable ingestion and activity classification | [Ingester guide](packages/agent-ingester/README.md) |
| `agent-server` | Queries, streams, mutation forwarding | [Server guide](packages/agent-server/README.md) |
| `agent-debug` | Offline replay and diagnostics | [Debug guide](packages/agent-debug/README.md) |

The extracted storage crate belongs to this workspace. The separately sourced `agent-parser`, `agent-watcher`, and `cli-command-kit` repositories remain native Cargo path dependencies beside this repository.

# Installation

The Rust migration is unpublished. Build an executable with `just debug build`, `just observer build`, `just ingester build`, or `just server build`. Each recipe copies its optimized executable into that package's `bin/` directory. Rust 1.95 or newer is required by the resolved Debug dependencies; verification used Rust 1.98.1, without a separate minimum-version run.

# Quick Start

```sh
just --list --list-submodules
just debug run --version
just debug run-ai session replay session.jsonl
just test
```

Root lifecycle recipes orchestrate package modules. Run `just <module> check` for a package's strict formatting, Clippy, tests, and coverage gate, or `just check` for all six. Cargo commands also accept native package selection, such as `cargo test --locked --package agent-debug`.

# Operational References

The CLI `skill` commands print installation pointers. The corresponding usage documents reside at repository-root paths:

- [Agent Debug](skills/agent-debug/SKILL.md)
- [Agent Observer](skills/agent-observer/SKILL.md)
- [Agent Ingester](skills/agent-ingester/SKILL.md)
- [Agent Server](skills/agent-server/SKILL.md)

Read the selected package's guide and skill before configuring live services. Native macOS runtime behavior, live harness/service interoperability, and external provider integration retain the verification limits recorded in the package guides. This workspace migration does not include the source repository's web frontend.

# License

[MIT](LICENSE), copyright 2026 Alex Gorbatchev. The inspected Agent Status Go source contained no license file; the migration uses the mandatory MIT default from the cli-best-practices skill. Existing package attribution is retained.
