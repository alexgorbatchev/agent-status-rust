`agent-ingester` receives coding-agent telemetry for Agent Status, records it durably, and publishes current activity to local consumers. Run it alongside `agent-observer` and the Agent Status server.

# What It Does

- **Durable delivery:** Acknowledges telemetry only after state, events, usage, and sequence checkpoints commit together.
- **Activity tracking:** Classifies root and subagent work, attention, rate limits, idle periods, completion, and process exits.
- **Optional summaries:** Produces short activity descriptions through OpenAI-compatible, Gemini, or Anthropic endpoints, with deterministic fallback.
- **Local control:** Publishes Unix-socket notifications and handles internal visibility, completion, and Herdr focus requests.

# How It Works

- Start the daemon with a bearer token matching the observer and server.
- Observers connect to `/ws/ingest` and send environment metadata and numbered telemetry frames.
- The daemon stores each accepted batch and acknowledges its highest durable sequence. Repeated acknowledged frames are skipped.
- Local subscribers receive environment, agent, and event notifications. The server reads the same database and forwards control requests.

# How it Really Works

- The default listener is `:4501`, accepting IPv4 and IPv6 when the host supports both. Configure a specific host to limit listening interfaces.
- The database is `$XDG_DATA_HOME/agent-status/agent-status.db`, falling back to `~/.local/share/agent-status/agent-status.db`. The notification socket is `$XDG_RUNTIME_DIR/agent-status/notify.sock`, falling back to `/tmp/agent-status/notify.sock`; its permissions are 0600.
- Reconciliation checks local processes at startup and every 30 seconds. Hourly maintenance removes telemetry older than 48 hours and agents older than 30 days, with associated stale usage and runs.
- Optional summaries send bounded, recursively scrubbed event context to configured providers. Provider billing applies. Each provider has a ten-second deadline; failures preserve deterministic summaries. Up to ten pending contexts per agent are retained, favoring newer work.
- Slow notification consumers are disconnected. An existing active socket or unrelated file at the configured path causes startup failure; it is preserved.
- Daemon stdout is empty. Diagnostics go to stderr and, by default, rotating disk logs. `AGENT=1` selects compact output; displayed home paths fold to `~`. Metadata and orderly shutdown exit 0; argument, startup, runtime, and output errors exit 1.
- SIGINT and SIGTERM stop accepting connections, cancel pending summaries, and join owned resources within a five-second shutdown deadline. Partially sent or unacknowledged telemetry remains replayable by observers.

# Installation

This Rust project is unpublished. The local release artifact is `bin/agent-ingester`; no Rust release download is available. The source provenance is [Agent Status](https://github.com/alexgorbatchev/agent-status).

# Setup

- Configure `AGENT_BEARER_TOKEN` and the observer's `AGENT_STATUS_TOKEN` with the same value. The source default is a fixed development token; configure your own token before exposing the listener.
- Set `AGENT_NO_SUMMARIZE=1` for local deterministic summaries. Otherwise configure provider credentials described in [the operational skill](../../skills/agent-ingester/SKILL.md); provider requests may incur charges.
- The Herdr multiplexer daemon is optional and needed only for focus requests. Set `HERDR_SOCKET_PATH` when its socket differs from `$XDG_CONFIG_HOME/herdr/herdr.sock` or `~/.config/herdr/herdr.sock`.

# Quick Start

```sh
agent-ingester --version
AGENT_NO_SUMMARIZE=1 agent-ingester --addr 127.0.0.1:4501
```

Sample Output for `--version`:

```text
0.1.0
```

In another terminal, `curl http://127.0.0.1:4501/healthz` returns `ok` followed by a newline.

# Options & Flags

Explicit flags override environment defaults.

| Flag | Short | Default | Description |
| :--- | :--- | :--- | :--- |
| `--addr <address>` | — | `AGENT_INGESTER_ADDR` or `:4501` | HTTP/WebSocket listen address with numeric port. |
| `--token <token>` | — | `AGENT_BEARER_TOKEN` or development token | Authentication for ingest and internal controls. |
| `--db <path>` | — | XDG data database | Shared SQLite database. |
| `--notify-sock <path>` | — | XDG runtime notification socket | Local subscriber socket. |
| `--no-summarize[=true|false]` | — | `AGENT_NO_SUMMARIZE=1` or false | Disable provider summaries; explicit false enables them. |
| `--litellm-url <url>` | — | Provider URL environment settings | OpenAI-compatible base URL. |
| `--litellm-key <key>` | — | Provider key environment settings | OpenAI-compatible API key. |
| `--summarizer-model <model>` | — | `AGENT_SUMMARIZER_MODEL` or per-provider default | Override every configured provider's model. |
| `--idle-grace-period <duration>` | — | `AGENT_IDLE_GRACE_PERIOD` or `5s` | Nonnegative delay before BUSY becomes IDLE after turn end. |
| `--log-dir <path>` | — | `AGENT_LOG_DIR` or XDG state logs | Rotating log directory. |
| `--log-file <path>` | — | `AGENT_LOG_FILE` or `ingester.log` | Exact rotating log filename. |
| `--log-level <level>` | — | `info` | debug, info, warn, warning, or error. |
| `--help` | `-h` | false | Print help. |
| `--version` | `-V` | false | Print raw version. |

`skill` prints the skill installation pointer. `completion bash|zsh|fish|powershell` prints a shell script. `help [COMMAND]...` prints command help. Their full interface, environment settings, API contracts, and examples are in [the operational skill](../../skills/agent-ingester/SKILL.md).

# Behavior and limits

Cross-environment WebSocket reassignment is rejected. Failed database updates return HTTP 500 rather than a misleading 404. Nested secret fields are scrubbed before summarization; malformed provider output falls back locally. Ambiguous model-price prefixes choose the most specific match deterministically. A new targetless tool clears stale target text, and terminal/error events clear in-flight state so late tool events cannot override attention or completion. Every background batch commits atomically. Maintenance timers receive fair scheduling.

The transport is HTTP/1.1 with WebSocket upgrades and a ten-second header deadline. Address ports are numeric. Herdr RPC responses are bounded to 64 MiB and must include the matching response ID and newline delimiter. Native macOS terminal activation is present but unverified at runtime on this Linux host; real local process, database, network, signal, and literal-subprocess-argument behavior is tested here. External provider and live Herdr interoperability require deployment validation.

# License

[MIT](LICENSE), copyright 2026 Alex Gorbatchev. The inspected Go source has no license file; this project uses the cli-best-practices skill's mandatory MIT default and retains source provenance.
