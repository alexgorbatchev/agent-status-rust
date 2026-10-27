---
created_on: 2026-10-14 13:43
last_modified: 2026-10-14 22:41
status: current
---

# Agent Server

Serve read-only Agent Status queries and forward explicit mutations to the ingester.

## Commands

- Test: `just test`.
- Format and lint: `just lint`.
- Coverage and full checks: `just check`.
- Release executable: `just build`, producing `bin/agent-server`.
- Run: `just run --addr 127.0.0.1:4502`; compact mode: `just run-ai --addr 127.0.0.1:4502`.
- Rust development needs the workspace packages `agent-common` and `agent-status-store`, and the standalone sibling `cli-command-kit` project and the LLVM tools required by cargo-llvm-cov. Consult the parent instructions before compiling or cleaning artifacts.

## Contracts

- Preserve read-only database opens and one transaction per query or snapshot. The ingester owns schema creation and writes; a missing database is a startup failure.
- Use the native shared models and usage aggregation. Preserve response field names and route-specific error statuses.
- Register event subscriptions before reading snapshots. Notification reconciliation can repeat updates because the source wire protocol has no sequence barrier.
- Keep global state-update overflow disconnects distinct from lossy log/live-tail overflow. Shared frames retain their original contents.
- Own and join HTTP connections, upgraded sockets, notification reconnects, and SQLite work. Streams have no REST deadline. Preserve the 10-second HTTP header, 15-second REST, 5-second proxy and shutdown deadlines.
- Mutable forwarding gets one transport attempt and no redirects. Preserve JSON bytes, encode an ID once as a path component, and buffer responses before exposing a successful status.
- Keep strict CSV flag parsing. Reject malformed quoting and extra records; size bounds follow the supplied argv value rather than document-size presets.
- Print errors once on stderr without usage. Preserve offline metadata output, raw version bytes, agent-mode help alerts, and shared home-path folding.
- Maintain `../../skills/agent-server/SKILL.md` in the same change as commands, arguments, flags, defaults, environments, outputs, or side effects. Update `last_modified`; compare it with the live command tree. The `skill` command prints the source installation pointer, not an embedded document.

## Verification and boundaries

- Any code change affecting runtime results requires a corresponding test-file change and at least 90% line coverage; `scripts/` is excluded.
- Record a behavioral failure, implement the correction, and temporarily disable it to prove the regression fails before restoring it.
- Use real local HTTP, WebSocket, Unix socket, subprocess, and database boundaries. Bound failure waits and isolate test environment directories under `.tmp/`.
- Never use live Agent Status services or unrelated credentials for tests. Never publish releases, tags, packages, or deployments without explicit user authorization.
- Keep source, evidence, delivered binaries, and lockfiles during reviewed artifact cleanup. Scope formatting to this package; `cargo fmt --all` visits sibling path dependencies.
- Persist only standing project rules here, updating them in place. Keep progress, handoffs, and test reports in `.tmp/`.
