`agent-debug` helps Agent Status operators investigate session transcripts, replayed activity, and stored telemetry. It provides offline diagnostics for Claude Code, Pi, Codex, and OpenCode, plus explicit server reads and optional live summarization.

# What It Does

- **Replay sessions:** Reconstruct agent activity, usage, and lifecycle state from transcripts or an OpenCode database.
- **Validate records:** Check telemetry framing, lifecycle termination, usage, and database read projections.
- **Inspect persisted state:** Read agents, historical runs, and individual events without modifying the monitoring database.
- **Audit discovery:** Pair native processes with session files and report orphan processes or sessions.
- **Compare results:** Show differences between replayed and persisted state, or group sessions by project and worktree.

# How It Works

- Give `session replay` a transcript path or session identity to inspect the reconstructed state.
- Use `session validate` for a pass/fail report, or `session view` for an activity card.
- Add `--export-db` to save replayed telemetry, then inspect it with `db inspect` and `event list`.
- Use `scanner audit` to investigate process/session pairing and `agent list` to locate active agents.
- Request remote observations with `agent usage`; enable provider calls explicitly with `session replay --summarize`.

# How it Really Works

- Replay normally keeps its telemetry in memory. `--export-db` creates or opens a writable SQLite database and commits processed frames incrementally; a failed replay can leave partial exported evidence. Monitoring and OpenCode input databases remain read-only.
- Replay supplies missing lifecycle boundaries by default. `--strict-lifecycle` preserves the transcript's boundaries. Replay accepts a missing termination event while still failing unrelated verification checks; validation requires termination. Database comparison treats only unended runs as current and keeps historical runs available for inspection.
- An exact session identity takes precedence over partial filename matches; ambiguous partial matches fail. Select `--session-id` when replaying an OpenCode database containing multiple sessions. Session discovery paths and database search order are documented in the [configuration reference](../../skills/agent-debug/references/setup.md).
- Local diagnostics operate without a running server. `agent list --all` and `agent usage` contact `AGENT_SERVER_HTTP_URL`, defaulting to `http://127.0.0.1:4502`. `AGENT_STATUS_TOKEN` supplies an optional bearer token. Requests have a ten-second deadline, one attempt, and zero redirects.
- Summarization is disabled by default and performs no provider environment lookup while disabled. `--summarize` enables configured OpenAI-compatible, Gemini, and Anthropic providers; calls may incur charges. Provider configuration and fallback order appear in the configuration reference.
- Results go to stdout; scanner warnings and returned failures go to stderr. `AGENT=1`, `true`, or `yes` selects compact output. `--json` supplies structured output where listed below. Displayed home paths use `~` and terminal control sequences are removed.
- Runtime commands also write rotating logs under `${XDG_STATE_HOME:-~/.local/state}/agent-status/logs`, with an intermediate `XDG_DATA_HOME` fallback. Defaults are 10 MiB, five compressed archives, and 14 days. `AGENT_LOG_TO_DISK=false` disables disk logging; help and version complete before logging starts.
- Successful commands exit 0. Argument, validation, cancellation, I/O, runtime, and output failures exit 1. Validation prints its report before returning a failed result. SIGINT and SIGTERM cancel current work and wait for owned tasks.

# Installation

This Rust workspace package has no published release artifact. Its build recipe targets `bin/agent-debug`; the matching operational reference is [skills/agent-debug/SKILL.md](../../skills/agent-debug/SKILL.md). The `skill` command prints an installation pointer to the source repository.

# Setup

- Provide an existing session transcript or OpenCode database for local replay. Set the harness-specific discovery overrides in the [configuration reference](../../skills/agent-debug/references/setup.md) when files live outside their usual locations.
- Supply an existing Agent Status database through `--db` for persisted inspection. `AGENT_DB_PATH` participates in session and fleet database discovery; event commands have their own documented search order.
- For remote reads, provide a running Agent Status server and set `AGENT_SERVER_HTTP_URL` and, when required, `AGENT_STATUS_TOKEN`.
- For optional summarization, configure a provider using the configuration reference before passing `--summarize`.

# Quick Start

```sh
agent-debug --version
AGENT=1 agent-debug session replay readme-session.jsonl
agent-debug session validate readme-session.jsonl --json
agent-debug session replay readme-session.jsonl --export-db replay.db --diff
agent-debug db inspect readme-session --db replay.db --json
agent-debug event list readme-session --db replay.db --asc --limit 0
agent-debug scanner audit --harness pi
```

Sample output from replaying a Pi transcript with session ID `readme-session`:

```text
harness:pi session_id:readme-session agent_id:env-debug:pi:readme-session project:repo branch: activity:BUSY lifecycle:EXITED(synthetic) tokens_in:0 tokens_out:0 tokens_total:0 cost_usd:0.0000 subagents:0 frames:3 verification:OK ver_logs:3 duration_ms:22
```

# Options & Flags

Boolean flags accept a bare flag as true or `--flag=true|false`. Command-specific options have no short aliases. Every command supports `--help` / `-h`.

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--debug[=bool]` | — | `false` | Enable diagnostic logs; accepted anywhere. |
| `--help` | `-h` | `false` | Print command help. |
| `--version` | `-V` | `false` | Print the raw version and newline at the root. |

`agent-debug session replay <file-or-session-id>`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--harness <name>` | — | automatic | `claude-code`, `pi`, `codex`, or `opencode`. |
| `--session-id <id>` | — | resolved identity | Select an OpenCode session or override the transcript identity. |
| `--cwd <path>` | — | transcript directory | Override the working directory and local Git resolution. |
| `--env <label>` | — | `debug` | Replay environment label. |
| `--json[=bool]` | — | `false` | Print the complete replay result as JSON. |
| `--strict-lifecycle[=bool]` | — | `false` | Preserve native lifecycle boundaries; alias `--no-synthetic-lifecycle`. |
| `--until-line <integer>` | — | `0` | Stop after source line N; zero processes all lines. |
| `--until-event <type>` | — | empty | Stop after the first matching telemetry event. |
| `--diff[=bool]` | — | `false` | Include state changes; print them incrementally outside JSON mode. |
| `--stop-on-error[=bool]` | — | `false` | Stop at the first parsing or ingestion failure. |
| `--summarize[=bool]` | — | `false` | Enable configured live summarization providers. |
| `--export-db <path>` | — | in memory | Save processed telemetry to SQLite. |
| `--db <path\|auto>` | — | no comparison | Compare replayed state with a monitoring database. |

`agent-debug session validate <file-or-session-id>`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--harness <name>` | — | automatic | Same harness values as replay. |
| `--session-id <id>` | — | resolved identity | Select or override the session identity. |
| `--cwd <path>` | — | transcript directory | Override the working directory. |
| `--env <label>` | — | `debug` | Replay environment label. |
| `--json[=bool]` | — | `false` | Print the validation report as JSON. |
| `--strict-lifecycle[=bool]` | — | `false` | Preserve native boundaries; alias `--no-synthetic-lifecycle`. |
| `--db <path\|auto>` | — | no comparison | Add a database discrepancy check. |

`agent-debug session view <file-or-session-id>`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--harness <name>` | — | automatic | Same harness values as replay. |
| `--session-id <id>` | — | resolved identity | Select or override the session identity. |
| `--cwd <path>` | — | transcript directory | Override the working directory. |
| `--env <label>` | — | `debug` | Replay environment label. |
| `--json[=bool]` | — | `false` | Print the reconstructed agent as JSON. |
| `--until-line <integer>` | — | `0` | Stop after source line N; zero processes all lines. |
| `--until-event <type>` | — | empty | Stop after the first matching telemetry event. |

`agent-debug fleet inspect [file-or-database-or-session-id...]`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--harness <name>` | — | automatic | Override transcript harness detection. |
| `--cwd <path>` | — | transcript directory | Override transcript working directories. |
| `--env <label>` | — | `debug` | Replay environment label. |
| `--json[=bool]` | — | `false` | Print an array of agents. |
| `--db <path\|auto>` | — | `auto` | Database used when no targets are supplied. |

`agent-debug event list <session-id>`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--db <path\|auto>` | — | `auto` | Monitoring database. |
| `--limit <integer>` | — | `20` | Maximum records; zero is unlimited. |
| `--type <CSV>` | — | empty | Filter telemetry event types. |
| `--subagent-id <id>` | — | empty | Filter a subagent identity. |
| `--include-subagents[=bool]` | — | `true` | Include subagent events. |
| `--desc[=bool]` | — | `true` | Show newest events first. |
| `--asc[=bool]` | — | `false` | Show oldest events first, overriding `--desc`. |
| `--json[=bool]` | — | `false` | Print an array with payload data and summaries. |

`agent-debug event inspect <positive-id>` and `agent-debug db inspect <session-id>`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--db <path\|auto>` | — | `auto` | Monitoring database. |
| `--json[=bool]` | — | `false` | Print the event or session record as JSON. |

`agent-debug scanner audit`, `agent-debug scanner list`, and `agent-debug process list`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--harness <name>` | — | all | Filter `claude`, `pi`, `codex`, or `opencode`. |
| `--cwd <path>` | — | all | Filter a directory and its descendants. |
| `--json[=bool]` | — | `false` | Print process/session pairing records as JSON. |

`agent-debug agent list`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--all[=bool]` | — | `false` | Query monitored machines from the server. |
| `--db <path>` | — | local discovery | Read local monitoring state; mutually exclusive with `--all`. |
| `--server-url <url>` | — | configured server URL | Override the server endpoint; requires `--all=true`. |

`agent-debug agent usage <agent-id>`

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--server-url <url>` | — | configured server URL | Override the server endpoint. |
| `--json[=bool]` | — | `false` | Print the complete usage report as JSON. |

`session list <file-or-database>` reads OpenCode session metadata and has no command-specific flags. `completion bash`, `completion zsh`, `completion fish`, and `completion powershell` print shell scripts. See the [operational skill](../../skills/agent-debug/SKILL.md) for full output contracts and generated help commands.

# Limits

Local fixture checks establish transcript, SQLite, HTTP, and process behavior. Live harness/server interoperability and native macOS execution remain unverified. The Rust executable and its operational skill are unpublished.

# License

[MIT](LICENSE), copyright 2026 Alex Gorbatchev.
