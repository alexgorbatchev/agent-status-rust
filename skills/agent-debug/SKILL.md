---
name: agent-debug
description: Use when replaying, validating, viewing, or auditing Agent Status sessions with agent-debug.
author: alexgorbatchev
metadata:
  created_on: 2026-10-14 15:01
  last_modified: 2026-10-14 15:15
  status: current
---

Use `AGENT=1` for compact results, `--json` for structured output where available, and native paths for inputs. Printed paths fold the home directory to `~`. Read [configuration](references/setup.md) for environment variables, discovery paths, logging, database search, and summarization providers.

## Workflow

```sh
AGENT=1 agent-debug session replay session.jsonl --json
AGENT=1 agent-debug session validate session.jsonl --json
AGENT=1 agent-debug session replay session.jsonl --export-db replay.db --diff
AGENT=1 agent-debug db inspect session --db replay.db --json
AGENT=1 agent-debug event list session --db replay.db --asc --limit 0 --json
```

Prefer a file path or exact session ID. Partial filename matches resolve only when unique, and the canonical resolved session identity is preserved. For multi-session OpenCode databases, select `--session-id`. Replay accepts an open-ended transcript while retaining failed verification checks; validation requires termination. Default replay synthesizes missing lifecycle boundaries; strict mode preserves native boundaries.

Read results on stdout and diagnostics on stderr. Metadata and successful commands exit 0; parsing, validation, cancellation, I/O, runtime, and output failures exit 1. SIGINT and SIGTERM cancel running work and drain owned tasks. JSON duration values are nanoseconds; timestamps are milliseconds unless a field explicitly formats UTC text. Database comparisons are read-only. `--export-db` creates replayed state and may leave partial committed evidence after failure. Summarization can send transcript-derived input to configured providers only when `--summarize` is enabled.

## Global options

Apply bool `--debug[=true|false]` anywhere, default false, for diagnostic logs on stderr. Every command supports bool `--help` / `-h`, default false. Root bool `--version` / `-V`, default false, prints the raw version and newline. Boolean command flags accept a bare flag as true or `--flag=true|false`; their default is false unless stated otherwise. There are no command-specific short aliases.

String-valued options use the native string type: `--harness`, `--session-id`, `--cwd`, `--env`, `--until-event`, `--export-db`, `--db`, `--type`, `--subagent-id`, and `--server-url`. Empty defaults select the behavior stated below. `--until-line` uses uint; `--limit` uses int64 with a nonnegative range.

## `agent-debug`

Print the root command tree when invoked without a subject.

## `agent-debug session list <file-or-database>`

Read every session in an OpenCode database, resolving a session ID only within OpenCode. Output a human table or agent records with session ID, title, directory, creation time, cost, and token total. There are no command-specific options.

## `agent-debug session replay <file-or-session-id>`

Replay through offline ingestion and verify server read projections. Options:

- `--harness <claude-code|pi|codex|opencode>`, default empty for auto detection.
- `--session-id <string>`, default empty; select an OpenCode session or override transcript identity.
- `--cwd <path>`, default empty; override transcript working directory and local Git resolution.
- `--env <string>`, default `debug`; replay environment label.
- `--json[=bool]`, default false; write the complete replay result instead of the summary.
- `--strict-lifecycle[=bool]`, visible alias `--no-synthetic-lifecycle`, default false; preserve native lifecycle boundaries.
- `--until-line <nonnegative integer>`, default 0 for all lines; stop after source line N.
- `--until-event <string>`, default empty; stop after the first matching telemetry event.
- `--diff[=bool]`, default false; include state changes, printed incrementally outside JSON mode.
- `--stop-on-error[=bool]`, default false; stop at the first parsing or ingestion failure.
- `--summarize[=bool]`, default false; enable configured live providers.
- `--export-db <path>`, default empty; write replayed telemetry to SQLite.
- `--db <path|auto>`, default empty; compare against persisted monitoring state when supplied.

## `agent-debug session validate <file-or-session-id>`

Validate harness detection, telemetry framing, lifecycle termination, nonnegative usage/pricing, and read queries. Options `--harness`, `--session-id`, `--cwd`, `--env`, `--json`, `--strict-lifecycle` / `--no-synthetic-lifecycle` have replay's types and defaults. `--db <path|auto>`, default empty, adds a database discrepancy check. Write the validation report before returning failure for failed checks.

## `agent-debug session view <file-or-session-id>`

Render the replayed state as a human card, compact record, or JSON agent. Options `--harness`, `--session-id`, `--cwd`, `--env`, `--json`, `--until-line`, `--until-event` have replay's types and defaults.

## `agent-debug fleet inspect [file-or-database-or-session-id]...`

Group replayed transcripts and persisted database agents by project and worktree. With no targets, read the monitoring database. Options `--harness`, `--cwd`, `--env`, `--json` have replay's types and defaults. `--db <path|auto>` defaults to `auto` and is used when targets are absent. Explicit `.db` targets load persisted monitoring agents; transcript targets run offline replay. JSON is an array of agents.

## `agent-debug event list <session-id>`

Read telemetry matching agent/session identifiers, including partial identifiers. Options:

- `--db <path|auto>`, default `auto`.
- `--limit <nonnegative integer>`, default 20; 0 means unlimited.
- `--type <CSV>`, default empty; select telemetry event types.
- `--subagent-id <string>`, default empty; filter a subagent.
- `--include-subagents[=bool]`, default true.
- `--desc[=bool]`, default true; newest first.
- `--asc[=bool]`, default false; oldest first, overriding `--desc`.
- `--json[=bool]`, default false; write an array including typed payload data and summaries.

## `agent-debug event inspect <id>`

Read one telemetry event by positive integer `id`, including subagent events. `--db <path|auto>` defaults to `auto`; `--json[=bool]` defaults false. JSON writes one object. An absent ID is a failure.

## `agent-debug db inspect <session-id>`

Read an agent, historical runs, event count, and event time window. Accept an exact session or full agent ID. `--db <path|auto>` defaults to `auto`; `--json[=bool]` defaults false. Database comparisons match active run IDs; ended runs remain historical records and do not count as an active-run discrepancy.

## `agent-debug scanner audit`

Capture live process metadata and session headers, pair process identities with sessions, and report paired/orphan rows. `--harness <claude|pi|codex|opencode>` defaults empty for all. `--cwd <path>` defaults empty; match that directory and descendants. `--json[=bool]` defaults false. Corrupt individual files produce path diagnostics on stderr while valid sessions remain; directory and database access errors fail the command.

## `agent-debug scanner list`

Run the same pairing audit with `scanner audit`'s options and defaults.

## `agent-debug process list`

Run the same pairing audit with `scanner audit`'s options and defaults.

## `agent-debug agent list`

Write headerless TSV: PID, harness, session ID, hostname, working directory, title. Without options, discover local native sessions. `--db <path>` selects monitored local state and is mutually exclusive with `--all`. `--all[=bool]`, default false, queries agent-server for monitored machines. `--server-url <HTTP(S) URL>` requires enabled `--all=true` and overrides the configured server URL. Keep running, unended positive-PID agents; local identities require matching process start time, remote identities require fresh heartbeat state.

## `agent-debug agent usage <agent-id>`

Query agent-server for explicit skill, command, and MCP observations. `--server-url <HTTP(S) URL>` overrides the configured server URL. `--json[=bool]`, default false, writes the complete report; text modes include coverage and history limits. Requests use one attempt, no redirects, and a ten-second deadline. Invalid status, malformed response, and mismatched response identity fail.

## `agent-debug skill`

Print the Skills CLI installation command and upstream skill URL, with identical human/agent bytes. Read this local reference for the independent Rust command interface.

## `agent-debug completion bash`

Write a Bash completion script to stdout.

## `agent-debug completion zsh`

Write a Zsh completion script to stdout.

## `agent-debug completion fish`

Write a Fish completion script to stdout.

## `agent-debug completion powershell`

Write a PowerShell completion script to stdout.

## `agent-debug help [command...]`

Print the selected command's help. Bare `session`, `fleet`, `event`, `db`, `scanner`, `process`, `agent`, and `completion` groups print their subtree help. Agent help starts with the skill-reading alert.

Read [generated help routes](references/help.md) for every accepted help path and its output contract.
