`agent-server` serves Agent Status queries and live updates for dashboard clients. It reads the ingester's existing database and forwards user actions to the ingester.

# What It Does

- **Query activity:** List environments, projects, agents, usage, and event history.
- **Stream changes:** Deliver snapshots and updates through SSE and WebSocket connections, with per-agent live tails.
- **Forward actions:** Change visibility, acknowledge or dismiss completion, and request terminal focus.
- **Control access:** Apply bearer authentication and configured browser origins.

# How It Works

- Start `agent-ingester` first so its database and notification socket exist.
- Start `agent-server` with the database, notification socket, and ingester HTTP URL.
- Connect your dashboard to the query endpoints and a live stream.
- Stop with Ctrl-C or SIGTERM; the service closes streams and joins its active work.

# How it Really Works

- The database is read-only. Its default path is `${XDG_DATA_HOME:-~/.local/share}/agent-status/agent-status.db`; starting the server does not create it. Mutations go to `http://127.0.0.1:4501` unless configured otherwise.
- Notifications arrive from `${XDG_RUNTIME_DIR:-/tmp}/agent-status/notify.sock`. The server reconnects every second and refreshes its snapshot after reconnecting. Malformed frames are logged without their raw telemetry contents.
- Each stream buffers 64 updates. A slow global client is disconnected when a state update cannot fit; log and live-tail updates may be dropped. Reconnect for a fresh snapshot. Snapshots and updates have no common sequence watermark, so updates can repeat during reconciliation.
- REST requests have a 15-second deadline and ingester forwarding has a five-second deadline. Streams remain open beyond the REST deadline. Mutation requests get one attempt, without redirect replay; unreadable upstream responses return 502. Request bodies retain the source service's unrestricted size policy.
- An empty token disables authentication. When configured, the token authenticates incoming requests and forwarded actions. Authorization headers take precedence over query tokens. Browser credentials require an allowed origin.
- Daemon stdout is empty; diagnostics go to stderr and a rotating `server.log` under `${XDG_STATE_HOME:-~/.local/state}/agent-status/logs`. `XDG_DATA_HOME` supplies the intermediate fallback when set. Defaults are 10 MiB, five compressed archives, and 14 days. `AGENT_LOG_TO_DISK=false` disables files.
- `AGENT=1`, `true`, or `yes` selects compact help and logs. Displayed home paths use `~`. Metadata output and orderly shutdown exit 0; argument, startup, runtime, and output failures exit 1. Shutdown has a five-second deadline; incomplete HTTP headers have a ten-second deadline.

# Installation

The local optimized executable is `bin/agent-server`. This Rust workspace package has no published release artifact. The skill installation pointer names the source repository; the matching Rust operational reference is [skills/agent-server/SKILL.md](../../skills/agent-server/SKILL.md).

# Setup

- Run an Agent Status ingester to provide the existing database, notification socket, and mutation endpoint.
- Set `AGENT_STATUS_TOKEN` to enable authentication and match the ingester's token.
- Configure `--cors-origin` for your dashboard's browser origin.

# Quick Start

```sh
agent-server --version
agent-server --addr 127.0.0.1:4502
AGENT=1 agent-server --ingester-url http://127.0.0.1:4501 --cors-origin http://localhost:3000
```

Sample output from `agent-server --version`:

```text
0.1.0
```

# Options & Flags

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--addr <host:port>` | `-a` | `:4502` | Listen address; port zero selects an available port. |
| `--db <path>` | — | XDG data path above | Existing read-only database. |
| `--socket <path>` | — | XDG runtime path above | Ingester notification socket. |
| `--ingester-url <url>` | — | `http://127.0.0.1:4501` | `AGENT_INGESTER_HTTP_URL` overrides the default. |
| `--cors-origin <origins>` | — | `http://localhost:3000` | Repeatable CSV; first occurrence replaces the default, later occurrences append. |
| `--token <token>` | — | empty | `AGENT_STATUS_TOKEN` overrides the default; an explicit empty flag disables authentication. |
| `--log-level <level>` | — | `info` | debug, info, warn, warning, error; case insensitive. |
| `--log-dir <path>` | — | shared logging directory | `AGENT_LOG_DIR` supplies an override. |
| `--log-file <path>` | — | `server.log` | `AGENT_LOG_FILE` supplies an override. |
| `--help` | `-h` | `false` | Print help. |
| `--version` | `-V` | `false` | Print raw version and newline. |

See the [operational skill](../../skills/agent-server/SKILL.md) for HTTP routes, stream semantics, log environment controls, and generated help/completion commands.

# License

[MIT](LICENSE), copyright 2026 Alex Gorbatchev.
