---
created_on: 2026-10-14 11:01
last_modified: 2026-10-14 22:41
status: current
---

# Agent observer

## Commands

- `just check`: scoped formatting, strict Clippy, tests, and at least 90% line coverage.
- `just build`: optimized binary at `bin/agent-observer`.
- `just run <args>` and `just run-ai <args>`: human and compact output.

## Contracts

- Keep discovery and transcript behavior in agent-watcher; observer owns environment identity, Git metadata, and acknowledged WebSocket delivery.
- Hold capacity for every unacknowledged frame, including disconnected frames. Reconnect preserves serialized bytes and sequence numbers. Invoke checkpoint callbacks in sequence order outside the shipper lock.
- Keep pending transcript checkpoints durable only after all events for each line are acknowledged. Join and flush watcher workers before stopping transport on shutdown.
- Pass path configuration directly rather than mutating environment variables. Treat harness databases and transcripts as read-only.
- Use native cancellation to kill and reap owned Git children. Do not contact Git remotes when resolving metadata.
- Keep origin-less project identities unique by canonical local root. Do not collapse different projects by basename.
- Keep bearer tokens and URL credentials out of logs, help defaults, and diagnostics. Fold paths only for display through the shared helper.
- Keep `../../skills/agent-observer/SKILL.md` synchronized in the same change as commands, flags, defaults, environment, outputs, or side effects. Update metadata and verify against the live Clap tree.
- The skill command prints only the compiled installation pointer. Never embed the usage skill or read repository files at runtime.

## Verification and boundaries

- Read applicable skills before editing. Runtime changes require corresponding behavioral test changes and at least 90% measured line coverage; scripts are excluded.
- Record a failing regression, implement it, then temporarily disable and restore the behavior to prove the tests detect it. Use real local WebSocket servers and subprocesses for transport and shutdown.
- Keep temporary files and evidence in `.tmp/`; scope formatting to this package and leave sibling repositories untouched.
- Persist standing guidance in place and keep progress/history outside agent instructions.
- Never publish releases, tags, packages, or production deployments without explicit authorization.
