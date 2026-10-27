---
name: agent-observer
description: Use when operating agent-observer to monitor local coding-agent sessions and deliver telemetry to an Agent Status ingester.
author: alexgorbatchev
metadata:
  created_on: 2026-10-14 11:01
  last_modified: 2026-10-14 11:11
  status: current
---

## Execution

Run the daemon on the machine that owns the agent sessions. Configure its
WebSocket ingester and matching bearer token first. Let SIGINT or SIGTERM stop
discovery, join readers, flush checkpoints, and stop the connection. Exit 0 means
metadata output or orderly shutdown; startup, argument, runtime, and output errors
exit 1. Errors appear once on stderr. Daemon stdout is empty.

Set AGENT=1, true, or yes (case and surrounding whitespace ignored) for compact
key-value logs and help. Human logs use timestamp and level tags. Displayed home
paths fold to ~; persisted paths and transmitted workspace paths remain absolute.
Daemon diagnostics go to stderr and, by default, a rotating disk log.

## `agent-observer`

Run discovery for Claude Code, Pi, Codex, and OpenCode. Accept zero positional
arguments. Send environment identity, project/workspace metadata, lifecycle,
transcript events, and token observations to the ingester. Session storage is
read-only. Local checkpoint files are written as ingester acknowledgements arrive.

| Flag | Short | Type | Default | Behavior |
| --- | --- | --- | --- | --- |
| --ingester-url | — | string | ws://127.0.0.1:4501/ws/ingest | Absolute ws or wss URL; AGENT_INGESTER_WS_URL overrides default. |
| --token | — | string | empty | Bearer token; AGENT_STATUS_TOKEN overrides default. |
| --env | — | string | empty | Environment label override. |
| --scan-interval | — | duration | 2s | Discovery interval. |
| --cursor-path | — | string | empty | Runtime default: XDG_CACHE_HOME/agent-status/observer-cursors.json. |
| --cursor-retention | — | duration | 24h | Inactive cursor retention; AGENT_CURSOR_RETENTION overrides default. |
| --cursor-prune-interval | — | duration | 1h | Pruning interval; AGENT_CURSOR_PRUNE_INTERVAL overrides default. |
| --cursor-flush-interval | — | duration | 2s | Persistence interval; AGENT_CURSOR_FLUSH_INTERVAL overrides default. |
| --session-state-path | — | string | empty | AGENT_SESSION_STATE_PATH, then XDG_CACHE_HOME/agent-status/observer-sessions.json. |
| --claude-sessions-dir | — | string | empty | Custom descriptor directory; then CLAUDE_SESSIONS_DIR or standard roots. |
| --claude-projects-dir | — | string | empty | Custom transcript directory or standard roots. |
| --pi-sessions-dir | — | string | empty | Custom Pi sessions directory or environment/default. |
| --codex-sessions-dir | — | string | empty | Custom Codex sessions directory or environment/default. |
| --opencode-db-path | — | string | empty | Custom OpenCode database or environment/default. |
| --log-level | — | string | info | debug, info, warn, warning, error; case-insensitive with surrounding whitespace ignored. Empty selects info. |
| --log-dir | — | string | empty | Override rotating-log directory. |
| --log-file | — | string | empty | Override exact rotating-log filename. |
| --help | -h | bool | false | Print help and exit. |
| --version | -V | bool | false | Print raw version and newline. |

Supply nonnegative durations using ns, us, µs, μs, ms, s, m, h, fractions, or
compound values such as 1h30m. Bare 0 selects the watcher's normal default for
the corresponding timer. Invalid duration environment defaults are ignored;
invalid explicit values are errors. Explicit flags override environment defaults.

### Identity and storage

Resolve the environment label from --env, AI_ENV, then the first nonempty
environment/env/label field in XDG_CONFIG_HOME/ai-registry/telemetry.json,
then hostname. XDG_CONFIG_HOME falls back to ~/.config. Identity hashes the
machine ID and user ID; hostname is the machine fallback. No token enters logs.

XDG_CACHE_HOME falls back to ~/.cache. OpenCode checkpoints use the cursor path
with .opencode appended. Session identity includes harness and session ID.
Checkpoint generations prevent old callbacks advancing replaced transcripts.

For Claude Code, CLAUDE_SESSIONS_DIR overrides descriptor discovery when nonempty;
otherwise search XDG_DATA_HOME/ai-registry/claude-code/sessions and ~/.claude/sessions.
CLAUDE_PROJECTS_DIR adds a transcript root before
XDG_DATA_HOME/ai-registry/claude-code/projects and ~/.claude/projects unless an
explicit --claude-projects-dir selects one directory.
For Pi use PI_SESSIONS_DIR, then PI_CODING_AGENT_DIR/sessions, then ~/.pi/agent/sessions.
For Codex use CODEX_SESSIONS_DIR, then CODEX_HOME/sessions, then ~/.codex/sessions.
For OpenCode use OPENCODE_DB_PATH, then XDG_DATA_HOME/opencode/opencode.db.
XDG_DATA_HOME falls back to ~/.local/share; HOME resolves the native home directory.

### Delivery and Git

Frames wait in a bounded queue until acknowledged. Reconnect retries pending
frames with the same sequence and payload; duplicate delivery is possible. Final
line checkpoints advance only after all events from the line are acknowledged.
Backoff starts at 500ms, grows to 30s with jitter, and resets after an ACK or five
seconds of connection health. Heartbeats run every ten seconds. Restart replays
transcripts from persisted checkpoints; unacknowledged in-memory queue entries
are recovered from source storage rather than a separate queue file.

Git on PATH resolves local origin, branch, worktree, and relative path; it never
contacts a Git remote. Cached metadata expires after 30s. Normalized SSH and HTTPS
origins identify one project. Projects without an origin use the canonical local
root hash, so identical directory names remain distinct. Failed Git lookup falls
back to a local project; cancellation stops and reaps the owned Git process.

### Logging

AGENT_LOG_TO_DISK=false, 0, no, or off disables disk output. AGENT_LOG_FILE sets
an exact filename; AGENT_LOG_DIR selects a directory. Default directory is
XDG_STATE_HOME/agent-status/logs, then XDG_DATA_HOME/agent-status/logs, then
~/.local/state/agent-status/logs; the filename is observer.log.
AGENT_LOG_MAX_SIZE_MB defaults to 10, AGENT_LOG_MAX_BACKUPS to 5,
AGENT_LOG_MAX_AGE_DAYS to 14, and AGENT_LOG_COMPRESS to true. Zero backups or age
means unlimited for that limit. The same false values disable compression.
CLI log paths override their environment settings. A disk-open failure warns
and continues on stderr; recorded write/flush errors make shutdown fail.

## `agent-observer skill`

Print the Skills CLI installation command and skill URL, with identical bytes
in both output modes. Accept zero arguments and command-specific flags. This
local Rust checkout is unpublished; install from its skills directory until the
source repository contains the matching operational reference.

## `agent-observer completion`

Print group help when no shell is supplied.

## `agent-observer completion bash`

Print Bash completion; source <(agent-observer completion bash).

## `agent-observer completion zsh`

Print Zsh completion; enable compinit before sourcing the generated script.

## `agent-observer completion fish`

Print Fish completion; agent-observer completion fish | source.

## `agent-observer completion powershell`

Print PowerShell completion; pipe to Out-String | Invoke-Expression.
Every shell command accepts zero positional arguments or command-specific flags.

## `agent-observer help [COMMAND]...`

Print root help or the named command's help, including help skill, help completion,
help completion bash, help completion zsh, help completion fish,
help completion powershell, and help help. The positional [COMMAND]... is an
optional command path. --help (-h) is a bool, default false, on every command.
Agent help begins with the skill-reading alert; raw version output remains plain.

### `agent-observer help skill`

Print skill-command help.

### `agent-observer help completion`

Print completion-group help.

### `agent-observer help completion bash`

Print Bash completion-command help.

### `agent-observer help completion zsh`

Print Zsh completion-command help.

### `agent-observer help completion fish`

Print Fish completion-command help.

### `agent-observer help completion powershell`

Print PowerShell completion-command help.

### `agent-observer help help`

Print help-command help.

### `agent-observer completion help`

Print completion-group help or help for a shell command named as its argument.

### `agent-observer completion help bash`

Print Bash completion-command help.

### `agent-observer completion help zsh`

Print Zsh completion-command help.

### `agent-observer completion help fish`

Print Fish completion-command help.

### `agent-observer completion help powershell`

Print PowerShell completion-command help.

### `agent-observer completion help help`

Print completion help-command help.

## Workflow

```sh
agent-observer --version
agent-observer --ingester-url ws://127.0.0.1:4501/ws/ingest --env laptop
AGENT=1 agent-observer --log-level debug --cursor-path ./observer-cursors.json
```

Configure AGENT_STATUS_TOKEN in the daemon's environment when its ingester
requires authentication. Keep the daemon running while coding agents work.
