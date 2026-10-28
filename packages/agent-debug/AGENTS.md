---
created_on: 2026-10-14 15:01
last_modified: 2026-10-14 22:41
status: current
---

# Agent Debug

Offline diagnostics for Agent Status telemetry, with explicit remote read and summarization operations.

## Commands

- `just check`: package formatting, strict Clippy, all targets, documentation tests, and 90% line coverage.
- `just build`: optimized executable at `bin/agent-debug`.
- `just run <args>` / `just run-ai <args>`: human / agent mode.
- Rust 1.95 or newer is required by resolved dependencies. Consult `Cargo.lock` before changing the minimum version.
- Scope formatters to this package.

## Contracts

- Maintain `../../skills/agent-debug/SKILL.md` and its references in the same change as any command, argument, option, default, environment, output, or side-effect change. Update `last_modified` and keep live command-interface tests current.
- Use native Clap parsing and subject-first domain commands. Framework metadata commands are the top-level exceptions.
- Keep stdout available for results; print a returned failure once on stderr. Raw version output is the version followed by one newline. All failures exit 1.
- Use `output::Format` for display sanitization and home folding; retain original native paths for filesystem operations and stored telemetry.
- Keep file and database reads off the async executor. Cancellation must join query monitors and blocking work. OpenCode reads use the native SQLite interrupt handle.
- Preserve canonical resolved session IDs; reject ambiguous partial IDs and paths that cannot be represented as UTF-8 telemetry strings.
- Keep corrupt per-file scanner diagnostics observable while retaining valid sessions. Genuine directory and database access errors remain failures.
- Replay may accept a missing explicit termination event; it must still reject every unrelated verification failure. Validation requires a completed lifecycle.
- Summarization is opt-in. Disabled summarization must perform no provider environment lookup or credential read.
- Monitoring and OpenCode databases are read-only inputs. Only `--export-db` writes replayed state. Preserve fallible observer and export outcomes.
- Use real local HTTP, SQLite, and process fixtures. Runtime-affecting changes require corresponding test-file changes and at least 90% line coverage; `scripts/` is excluded. Record red/green and genuine implementation-disable/restoration evidence under `.tmp/`.
- Never publish releases, tags, packages, or production deployments automatically without explicit user authorization.
- Persist standing rules in place; keep progress and dated handoffs outside this file.

## Provenance

The source contract is `alexgorbatchev/agent-status`, package `agent-debug`. The source checkout contains no license file; this workspace uses the cli-best-practices skill's mandatory MIT default with Alex Gorbatchev attribution. Shared Agent Status behavior resides in workspace packages; `agent-parser`, `agent-watcher`, and `cli-command-kit` resolve from pinned GitHub revisions.
