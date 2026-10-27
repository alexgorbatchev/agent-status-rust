---
name: agent-server
description: Use when operating agent-server to serve Agent Status queries and live streams.
author: alexgorbatchev
metadata:
  created_on: 2026-10-14 13:35
  last_modified: 2026-10-14 13:51
  status: current
---

## `agent-server`

Run the HTTP service against the database written by `agent-ingester`. Read routes use read-only SQLite transactions; mutations go to the configured ingester. The database must already exist. Startup and diagnostics go to stderr; the daemon leaves stdout empty. SIGINT or SIGTERM requests shutdown. Exit 0 means successful metadata output or shutdown; exit 1 means argument, startup, service, logging, or output failure.

| Option | Type | Default and behavior |
| --- | --- | --- |
| `--addr`, `-a` | string | `:4502`; host:port or :port; port 0 requests an available port |
| `--db` | path | `$XDG_DATA_HOME/agent-status/agent-status.db`, otherwise `~/.local/share/agent-status/agent-status.db` |
| `--socket` | path | `$XDG_RUNTIME_DIR/agent-status/notify.sock`, otherwise `/tmp/agent-status/notify.sock` |
| `--ingester-url` | string | `$AGENT_INGESTER_HTTP_URL`, otherwise `http://127.0.0.1:4501` |
| `--cors-origin` | CSV string value, repeatable | `http://localhost:3000`; first occurrence replaces the default, subsequent occurrences append; quote comma-containing fields using CSV double quotes; malformed quoting or multiple records fails validation |
| `--token` | string | `$AGENT_STATUS_TOKEN`, otherwise empty; empty disables authentication; an explicit empty flag overrides the environment |
| `--log-level` | string | `info`; debug, info, warn, warning, error; case insensitive |
| `--log-dir` | path | `$AGENT_LOG_DIR`, otherwise the shared logging state directory |
| `--log-file` | path | `$AGENT_LOG_FILE`, otherwise `server.log` in the log directory |
| `--help`, `-h` | bool | Print help and exit |
| `--version`, `-V` | bool | Print the raw package version and newline |

Set `AGENT=1`, `true`, or `yes` for compact diagnostics and help. Displayed home-directory paths use `~`. `XDG_STATE_HOME` selects the default disk-log state directory; `AGENT_LOG_TO_DISK=false`, `0`, or `no` disables disk logging. `AGENT_LOG_MAX_SIZE_MB`, `AGENT_LOG_MAX_BACKUPS`, `AGENT_LOG_MAX_AGE_DAYS`, and `AGENT_LOG_COMPRESS` configure the shared rotating log policy. Path variables preserve native filesystem paths; non-UTF-8 token or ingester-URL environment values fail validation.

Example:

```sh
AGENT=1 agent-server --addr 127.0.0.1:4502 --cors-origin http://localhost:3000
```

Authenticate requests with `Authorization: Bearer <token>`, or the `token` / `access_token` query parameter. A supplied Bearer value takes precedence. An authenticated service forwards its configured token to the ingester. With authentication disabled it forwards an incoming Authorization header, and excludes query credentials from forwarded URLs.

## HTTP queries

Read JSON at `/api/v1/status`, `/api/v1/projects`, `/api/v1/environments`, `/api/v1/agents`, `/api/v1/agents/{id}`, `/api/v1/agents/{id}/usage`, and `/api/v1/agents/{id}/logs`.

Filter agents with `project`, `environment`, `lifecycle`, `activity`, and `hidden=true|false`. Logs accept positive `limit` (default 100, maximum 1000), positive `before` event ID, and `subagent` (empty selects top-level events). Encode an agent ID as one URL path component. Unknown agents return 404 on inspection and usage; failed database queries return 500 with a public error message.

Send `PATCH /api/v1/agents/{id}` with `isHidden` boolean, `dismissCompletion=true`, or `acknowledgeCompletion=true`. Unknown JSON fields pass through; recognized false completion flags alone fail validation. Unreadable or invalid request bodies return 400 JSON. Bodies have no configured size ceiling. Send `POST /api/v1/agents/{id}/focus` to focus the associated terminal. Both go to `/api/v1/internal/agents/{id}` on the ingester, with `/focus` for focus. Upstream status and JSON body are returned. Transport or truncated-response failures return 502; REST requests have a 15-second deadline, forwarding has a 5-second deadline. Mutable requests get one attempt; redirect responses return without replay.

## Live streams

Open `/api/v1/events` for SSE snapshots and ServerEvent updates, `/api/v1/agents/{id}/live-tail` for that agent's telemetry frames, or `/ws/events` for WebSocket snapshots and ServerEvent updates. SSE heartbeats arrive every 15 seconds. Streams remain open beyond the REST deadline. WebSocket origins permit same-host requests and configured patterns; unexpected application data closes with policy code 1008. Native Ping/Pong remains supported.

Each subscriber buffers 64 updates. A slow global subscriber is disconnected when a state update cannot fit; log updates and live-tail frames may be dropped. Reconnect for a fresh snapshot. The ingester notification socket reconnects every second and reconciles a database snapshot on hello or the first ordinary event. Subscription precedes snapshot reads to retain changes arriving during the read; the unsequenced protocol can repeat updates around reconciliation.

Notification lines are bounded at 16 MiB; a larger line ends that connection and triggers reconnect. HTTP headers have a ten-second read deadline. Shutdown cancels streams and database reads, stops admission, and waits up to five seconds for owned work; a missed deadline returns an error.

## `agent-server skill`

Print the Skills CLI install command and source skill URL. This metadata command accepts no arguments or command-specific options and performs no service or filesystem work.

## `agent-server help [command]`

Print help for the root or the selected metadata command. Every agent-mode help screen begins with the skill alert.

Generated help paths include `agent-server help skill`, `agent-server help help`, `agent-server help completion`, `agent-server help completion bash`, `agent-server help completion zsh`, `agent-server help completion fish`, and `agent-server help completion powershell`. The completion group also accepts `agent-server completion help`, `agent-server completion help bash`, `agent-server completion help zsh`, `agent-server completion help fish`, `agent-server completion help powershell`, and `agent-server completion help help`.

## `agent-server completion`

Print completion-command help.

## `agent-server completion bash`

Print Bash completion script.

## `agent-server completion zsh`

Print Zsh completion script.

## `agent-server completion fish`

Print Fish completion script.

## `agent-server completion powershell`

Print PowerShell completion script.
