---
name: agent-ingester
description: Use when operating agent-ingester to receive, classify, persist, and publish Agent Status telemetry.
author: alexgorbatchev
metadata:
  created_on: 2026-10-14 12:27
  last_modified: 2026-10-14 12:27
  status: current
---

## Execution

Run the daemon with a token matching agent-observer's AGENT_STATUS_TOKEN and
the server's ingester token. SIGINT or SIGTERM stops acceptance, cancels pending
summaries, and joins owned resources with a five-second deadline. Exit 0 means
metadata output or orderly shutdown; argument, startup, runtime, and output
errors exit 1. Errors appear once on stderr; daemon stdout is empty.

Set AGENT=1, true, or yes, ignoring case and surrounding whitespace, for compact
logs and help. Human logs use timestamp and level tags. Displayed home paths
fold to ~; database and transmitted paths retain their actual values.

## `agent-ingester`

Start the HTTP/WebSocket telemetry daemon. Accept zero positional arguments.
Explicit flags override environment settings. Metadata commands finish before
loading daemon configuration or opening files or network listeners.

| Flag | Short | Type | Default | Behavior |
| --- | --- | --- | --- | --- |
| --addr | — | string | :4501 | AGENT_INGESTER_ADDR overrides default; numeric port required. |
| --token | — | string | development token | AGENT_BEARER_TOKEN overrides default; authenticate observers and internal controls. |
| --db | — | string | empty | Runtime default: XDG_DATA_HOME/agent-status/agent-status.db, then ~/.local/share/agent-status/agent-status.db. |
| --notify-sock | — | string | empty | Runtime default: XDG_RUNTIME_DIR/agent-status/notify.sock, then the system /tmp directory's agent-status/notify.sock. |
| --no-summarize | — | bool | false | AGENT_NO_SUMMARIZE=1 disables summaries; bare flag means true; --no-summarize=false explicitly enables them. |
| --litellm-url | — | string | empty | LITELLM_BASE_URL, then OPENAI_BASE_URL; OpenAI-compatible base URL. |
| --litellm-key | — | string | empty | LITELLM_API_KEY, then LITELLM_KEY, then OPENAI_API_KEY. |
| --summarizer-model | — | string | empty | AGENT_SUMMARIZER_MODEL, then provider defaults; applies to all configured providers. |
| --idle-grace-period | — | duration | 5s | AGENT_IDLE_GRACE_PERIOD overrides default; delay from terminal root turn end to IDLE. |
| --log-dir | — | string | empty | AGENT_LOG_DIR or the default rotating-log directory. |
| --log-file | — | string | empty | AGENT_LOG_FILE or ingester.log; exact filename. |
| --log-level | — | string | info | debug, info, warn, warning, error; case-insensitive, surrounding whitespace ignored. Empty selects info. |
| --help | -h | bool | false | Print help and exit. |
| --version | -V | bool | false | Print raw version and newline. |

Supply nonnegative durations with ns, us, µs, μs, ms, s, m, h, fractions, or
compound values such as 1h30m. Bare 0 means zero grace. Invalid environment
duration falls back to 5s and logs an error; invalid explicit values fail offline.
An empty explicit token disables internal HTTP authentication; WebSocket ingest
still requires an Authorization header with the Bearer prefix.

### Persistence and notifications

Observers connect to /ws/ingest using Bearer authentication and version 1.0
frames. hello binds the connection to an environment. frames advances its
sequence cursor only in the transaction that commits state, events, usage, and
heartbeat; ACK is sent afterward as {"type":"ack","upTo":N}. Replayed durable
sequences are skipped. Invalid or failed batches return typed error messages
without ACK; a connection keeps its original environment identity.

The notification socket sends newline-delimited JSON, beginning with hello,
then environment_upsert, agent_upsert, and log_append events. A slow reader is
disconnected. Existing active sockets and unrelated paths are preserved.
Reconciliation checks local processes at startup and every 30s. Hourly cleanup
prunes telemetry older than 48h and agents older than 30d, with stale runs/usage.

Internal HTTP routes use the configured bearer token: PATCH
/api/v1/internal/agents/{id}/visibility accepts {"isHidden":bool}; PATCH or POST
.../{id}/dismiss-completion acknowledges completion. PATCH .../{id} accepts any
combination of isHidden, dismissCompletion, acknowledgeCompletion boolean fields.
Successful mutations return the updated agent and publish its notification.
Malformed/empty changes return 400; missing agents 404; database failures 500.
POST .../{id}/focus invokes Herdr. It returns ok, paneId, workspaceId, tabId on
success; 404 for missing agent/pane, 409 for an exited agent, 503 for unavailable
Herdr, and 500 for lookup/focus/database failures. /healthz returns ok plus newline
without authentication, for all HTTP methods.

### Providers and logging

OpenAI-compatible configuration is selected when its URL or key is nonempty;
default model is gpt-4o-mini. GEMINI_API_KEY enables gemini-2.0-flash;
ANTHROPIC_API_KEY enables claude-3-5-haiku-latest. Providers are tried in that
order, each with a 10s deadline. Failed/malformed responses use the next provider
or deterministic fallback. Summaries are debounced for 10s, serial per agent,
parallel across agents, with ten pending latest contexts. Rate-limited activity
uses a fixed local description. Context is bounded and recursively scrubbed.

HERDR_SOCKET_PATH selects Herdr's socket; otherwise use
XDG_CONFIG_HOME/herdr/herdr.sock, then ~/.config/herdr/herdr.sock. Pane matching
prefers exact positive PID, then cleaned cwd plus harness name. macOS optionally
activates HERDR_TERMINAL_APP or TERM_PROGRAM after successful focus. Activation
is best effort and has no native macOS runtime verification in this Linux build.

AGENT_LOG_TO_DISK=false, 0, no, or off disables disk output. AGENT_LOG_FILE sets
an exact filename; AGENT_LOG_DIR selects a directory. Default directory is
XDG_STATE_HOME/agent-status/logs, then XDG_DATA_HOME/agent-status/logs, then
~/.local/state/agent-status/logs; filename ingester.log. AGENT_LOG_MAX_SIZE_MB
defaults to 10, AGENT_LOG_MAX_BACKUPS to 5, AGENT_LOG_MAX_AGE_DAYS to 14, and
AGENT_LOG_COMPRESS to true. Zero backups or age means unlimited for that limit.
The same false values disable compression. CLI paths override environment.
Disk-open failures warn and continue on stderr; write/flush errors fail shutdown.

## Workflow

```sh
agent-ingester --version
AGENT_NO_SUMMARIZE=1 agent-ingester --addr 127.0.0.1:4501
```

Configure AGENT_BEARER_TOKEN in the daemon environment and the matching token in
its observer/server consumers. Optional provider requests may incur billing.

## `agent-ingester skill`

Print the Skills CLI installation command and skill URL, with identical bytes
in both output modes. Accept zero arguments and command-specific flags. This
local Rust checkout is unpublished; install from its skills directory until the
source repository contains the matching operational reference.

## `agent-ingester completion`

Print group help when no shell is supplied.

## `agent-ingester completion bash`

Print Bash completion; source <(agent-ingester completion bash).

## `agent-ingester completion zsh`

Print Zsh completion; enable compinit before sourcing the generated script.

## `agent-ingester completion fish`

Print Fish completion; agent-ingester completion fish | source.

## `agent-ingester completion powershell`

Print PowerShell completion; pipe to Out-String | Invoke-Expression.
Every shell command accepts zero positional arguments or command-specific flags.

## `agent-ingester help [COMMAND]...`

Print root help or the named command's help, including help skill, help completion,
help completion bash, help completion zsh, help completion fish,
help completion powershell, and help help. The positional [COMMAND]... is an
optional command path. --help (-h) is a bool, default false, on every command.
Agent help begins with the skill-reading alert; raw version output remains plain.

### `agent-ingester help skill`

Print skill-command help.

### `agent-ingester help completion`

Print completion-group help.

### `agent-ingester help completion bash`

Print Bash completion-command help.

### `agent-ingester help completion zsh`

Print Zsh completion-command help.

### `agent-ingester help completion fish`

Print Fish completion-command help.

### `agent-ingester help completion powershell`

Print PowerShell completion-command help.

### `agent-ingester help help`

Print help-command help.

### `agent-ingester completion help`

Print completion-group help or help for a shell command named as its argument.

### `agent-ingester completion help bash`

Print Bash completion-command help.

### `agent-ingester completion help zsh`

Print Zsh completion-command help.

### `agent-ingester completion help fish`

Print Fish completion-command help.

### `agent-ingester completion help powershell`

Print PowerShell completion-command help.

### `agent-ingester completion help help`

Print completion help-command help.

