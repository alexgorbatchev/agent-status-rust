---
created_on: 2026-10-14 12:27
last_modified: 2026-10-14 22:41
status: current
---

# Agent ingester

## Commands

- `just check`: package formatting, strict Clippy, all tests, and at least 90% line coverage.
- `just build`: optimized binary at `bin/agent-ingester`.
- `just run <args>` and `just run-ai <args>`: human and compact output.

## Contracts

- Keep schema and database primitives in agent-status-store. Keep classifier transitions independent of transport and wall-clock acquisition.
- Commit agent state, raw telemetry, usage, environment heartbeat, and sequence cursor in one transaction before sending an ACK. Restore classifier context on transaction failure; failed batches emit neither ACK nor state notifications.
- Bind a WebSocket connection to its first environment identity. Preserve replay deduplication and observer generation semantics.
- Read fresh rows for summaries, timers, reconciliation, and HTTP mutations. Apply all background updates atomically. Reject summaries from an old context generation or run.
- Keep completion sticky until acknowledged, dismissed, or replaced by a new root turn. Attention states outrank BUSY; terminal/error events clear in-flight state.
- Keep summary workers serial per agent, parallel across agents, with ten pending latest-context jobs. Cancel and join owned tasks on shutdown. Schedule classifier deadlines through the owner's watch channel; use fair selection across maintenance timers.
- Native HTTP header deadlines are per connection. Keep graceful HTTP, WebSocket, Unix notification, dispatcher, and database shutdown bounded and owned.
- Refuse to unlink unrelated files, symlinks, active sockets, or replacement socket inodes. Notification backpressure evicts slow consumers rather than blocking ingestion.
- Scrub excluded prompt keys recursively. Never print credentials, raw bearer tokens, or credential-bearing endpoint URLs. Pass terminal application names as subprocess arguments rather than AppleScript source.
- Keep `../../skills/agent-ingester/SKILL.md` synchronized in the same change as commands, arguments, flags, defaults, environment, outputs, or side effects. Update metadata and compare it with the live command tree.
- The skill command prints a compiled installation pointer, without reading repository files at runtime.

## Verification and boundaries

- Read applicable skills before editing. Any runtime change requires a corresponding behavioral test-file change and at least 90% measured line coverage; scripts are excluded.
- Record red/green regressions and genuine implementation-disable failures, then restore source bytes and rerun tests. Use real local SQLite, HTTP/WebSocket/Unix servers, and subprocesses.
- Keep temporary evidence in `.tmp/`; scope formatters to this package and preserve sibling ownership.
- Keep standing instructions here and progress/history outside AGENTS.md.
- Never publish releases, tags, packages, or production deployments without explicit authorization. Do not contact live agent services or language-model providers during verification.
