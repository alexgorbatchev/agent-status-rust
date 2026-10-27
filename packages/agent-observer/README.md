`agent-observer` monitors coding-agent sessions on your machine and delivers their activity to an Agent Status ingester. It attaches environment, project, branch, and workspace metadata so the dashboard can group work across machines.

# What It Does

- **Watch four harnesses:** Follow Claude Code, Pi, Codex, and OpenCode session activity.
- **Identify workspaces:** Resolve local Git origins, branches, worktrees, and distinct projects without origins.
- **Recover delivery:** Replay unacknowledged events after reconnecting and persist acknowledged transcript positions.
- **Stop cleanly:** Join discovery and transcript workers, flush checkpoints, and stop shipping on SIGINT or SIGTERM.

# How It Works

- Start the daemon on the machine running your coding agents.
- Set the ingester WebSocket URL and matching authentication token.
- Leave it running while agents work; activity appears through your ingester and dashboard.
- Stop it with Ctrl-C or SIGTERM. Restart uses saved checkpoints.

# How it Really Works

- Session transcripts and OpenCode databases are read-only. Cursor and session state default to `${XDG_CACHE_HOME:-~/.cache}/agent-status/observer-cursors.json` and `observer-sessions.json`; OpenCode uses a companion `.opencode` checkpoint file.
- Every transmitted frame includes environment and session identity, harness, workspace, event data, and timestamp. The ingester receives transcript-derived text and tool metadata; select a trusted endpoint.
- A line's checkpoint advances after every event derived from it is acknowledged. Reconnect can deliver duplicates. A full queue slows transcript reading instead of dropping events; restarting recovers unacknowledged data from source transcripts.
- Git metadata comes from local commands and expires from the cache after 30 seconds. Git remotes are never contacted. Projects without an origin use distinct canonical-root identities, even when their directories share a name.
- Logs go to stderr and a rotating `observer.log` under `${XDG_STATE_HOME:-~/.local/state}/agent-status/logs`. `XDG_DATA_HOME` supplies the intermediate fallback when set. Rotation defaults to 10 MiB, five compressed archives, and 14 days; `AGENT_LOG_TO_DISK=false` disables files.
- Daemon stdout is empty. Help, version, skill installation information, and completion scripts use stdout. `AGENT=1`, `true`, or `yes` selects compact help and logs. Displayed home paths use `~`.
- Metadata output and orderly shutdown exit 0; invalid arguments, startup, runtime, and output failures exit 1. A disk-open failure warns and continues on stderr; recorded log write or flush errors fail shutdown.

# Installation

The local optimized executable is `bin/agent-observer`. This Rust workspace package has no published release artifact. Its skill installation pointer names the source repository; the matching Rust operational reference is available locally at [skills/agent-observer/SKILL.md](../../skills/agent-observer/SKILL.md).

# Setup

- Run an Agent Status ingester and supply its `ws://` or `wss://` ingestion endpoint with `--ingester-url` or `AGENT_INGESTER_WS_URL`.
- Set `AGENT_STATUS_TOKEN` to the token expected by that ingester.
- Install [Git](https://git-scm.com/) on PATH for workspace metadata. Missing Git falls back to local directory identities.

# Quick Start

```sh
agent-observer --version
agent-observer --ingester-url ws://127.0.0.1:4501/ws/ingest --env laptop
AGENT=1 agent-observer --log-level debug
```

Sample output from `agent-observer --version`:

```text
0.1.0
```

# Options & Flags

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--ingester-url <url>` | — | `ws://127.0.0.1:4501/ws/ingest` | Absolute WebSocket URL; environment override supported. |
| `--token <token>` | — | empty | Bearer token; `AGENT_STATUS_TOKEN` overrides the default. |
| `--env <label>` | — | empty | Label; otherwise `AI_ENV`, telemetry config, then hostname. |
| `--scan-interval <duration>` | — | `2s` | Discovery interval. |
| `--cursor-path <path>` | — | empty | Use the observer cache default unless supplied. |
| `--cursor-retention <duration>` | — | `24h` | Inactive cursor retention. |
| `--cursor-prune-interval <duration>` | — | `1h` | Cursor pruning interval. |
| `--cursor-flush-interval <duration>` | — | `2s` | Cursor persistence interval. |
| `--session-state-path <path>` | — | empty | Use environment or observer cache default. |
| `--claude-sessions-dir <path>` | — | empty | Claude Code descriptor directory. |
| `--claude-projects-dir <path>` | — | empty | Claude Code transcript directory. |
| `--pi-sessions-dir <path>` | — | empty | Pi sessions directory. |
| `--codex-sessions-dir <path>` | — | empty | Codex sessions directory. |
| `--opencode-db-path <path>` | — | empty | OpenCode SQLite database. |
| `--log-level <level>` | — | `info` | debug, info, warn, warning, error; case-insensitive and trimmed. Empty selects info. |
| `--log-dir <path>` | — | empty | Override rotating-log directory. |
| `--log-file <path>` | — | empty | Override exact rotating-log filename. |
| `--help` | `-h` | `false` | Print help. |
| `--version` | `-V` | `false` | Print raw version and newline. |

Durations accept nonnegative Go-style values such as `250ms`, `1.5s`, and `24h`; zero selects the corresponding watcher's default. Invalid explicit values and extra positional arguments are errors. See the [operational skill](../../skills/agent-observer/SKILL.md) for environment precedence, storage paths, log controls, and `completion`/`help`/`skill` commands.

# License

[MIT](LICENSE), copyright 2026 Alex Gorbatchev.
