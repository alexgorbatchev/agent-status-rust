`agent-common` provides telemetry data contracts, usage evidence extraction, context-capacity estimates, and structured logging for agent monitoring services.

# What It Does

- **Telemetry contracts**: Serializes agent, observer, ingester, and server messages with their established JSON field names.
- **Observed usage**: Extracts explicit skill loads, skill invocations, slash commands, and MCP calls with stable replay identities.
- **Context estimates**: Resolves model capacities and calculates utilization from token counts.
- **Service logging**: Writes human output or machine logfmt with optional compressed disk rotation.

# How It Works

1. Deserialize telemetry into typed messages and pass frames to `usage::extract`.
2. Store the returned observations by their IDs to deduplicate replayed calls.
3. Construct a logger and run service work with its dispatch installed.
4. Call `finish()` to flush output, close disk logs, and receive recorded I/O errors.

# How it Really Works

- Extraction is synchronous and reads only the supplied frame. It excludes conversational mentions, skill catalogs, shell commands, and tool results. Native call IDs survive timestamp and transport-sequence changes; observations without call IDs can undercount identical events at the same timestamp.
- Snapshot messages always serialize `agents` and `environments` as arrays. Optional completion fields preserve the distinction between absent values and explicit `false` or zero. Canonical camelCase completion fields take precedence over the established snake_case input fields.
- Context resolution is an estimate. The longest catalog prefix wins; unknown models use a 128,000-token fallback. A `-1m` or `:1m` suffix represents caller configuration and does not prove provider support. Utilization is not clamped to one.
- Log writes and gzip compression are synchronous. A shared logger serializes whole records across threads. Its dispatch must be installed in each thread or asynchronous task that uses it; constructing a logger does not replace global logging.
- Disk initialization failures produce a warning and retain console output. Later write and flush failures remain available through `finish()`. Dropping a logger closes its disk writer but cannot return failures.
- Disk files use `service_rCURRENT.log` and timestamped archives such as `service_r2026-10-14_13-00-00.log.gz`. Backup-count and age limits apply independently; explicit zero disables the corresponding limit. Files belonging to other services are retained.
- No network requests run, and no filesystem work occurs merely by importing the crate. Configuration reads happen at explicit construction or environment-resolution calls.

# Installation

From a consumer project beside this local checkout:

```bash
cargo add agent-common --path ../agent-status/packages/agent-common
```

# Quick Start

```rust
use agent_common::{logging::{FileConfig, Logging, Mode, Options}, models};

fn main() -> std::io::Result<()> {
    let logger = Logging::setup(
        std::io::stderr(),
        Options { service: "observer".into(), level: "info".into(), mode: Mode::Agent },
        FileConfig { disabled: true, ..Default::default() },
    )?;
    logger.with_default(|| {
        tracing::info!(capacity = models::resolve_context_window("gpt-5.4"), "started");
    });
    logger.finish()
}
```

Consumers using the event macros also add `tracing` with `cargo add tracing`.

# Configuration

`FileConfig::default()` supplies defaults; `FileConfig::from_env()` additionally reads the disk options below. `Options::default()` reads `AGENT` through `Mode::from_env()`. Explicit options override these resolved values.

| Variable | Default | Meaning |
| :--- | :--- | :--- |
| `AGENT` | human | `1`, `true`, or `yes`, ignoring case and surrounding whitespace, selects agent output. |
| `AGENT_LOG_TO_DISK` | enabled | `0`, `false`, `no`, or `off` disables disk logging. |
| `AGENT_LOG_FILE` | derived | File specification; its stem receives the current-file or archive infix. |
| `AGENT_LOG_DIR` | XDG location | Directory used when no file is specified. |
| `AGENT_LOG_MAX_SIZE_MB` | `10` | Positive size threshold in MiB. |
| `AGENT_LOG_MAX_BACKUPS` | `5` | Archive count, or `0` for unlimited. |
| `AGENT_LOG_MAX_AGE_DAYS` | `14` | Archive age, or `0` for unlimited. |
| `AGENT_LOG_COMPRESS` | enabled | `0`, `false`, `no`, or `off` keeps plain archives. |
| `XDG_STATE_HOME` | unset | First fallback: `agent-status/logs` below this directory. |
| `XDG_DATA_HOME` | unset | Second fallback: `agent-status/logs` below this directory. |

The final directory fallback is `~/.local/state/agent-status/logs`, or the OS temporary directory's `agent-status/logs` when a home directory cannot be resolved. `parse_level` accepts `debug`, `info`, `warn`, `warning`, and `error`; an empty level means `info`. Invalid levels return an input error.

# API

| Entry point | Result |
| :--- | :--- |
| `usage::extract(&TelemetryFrame)` | Explicit observations with stable IDs and evidence. |
| `usage::Report::new(agent_id, harness)` | Empty usage arrays and extractor-coverage descriptions. |
| `models::resolve_context_window(model_id)` | Capacity estimate in tokens. |
| `models::calculate_context_fullness(used, capacity)` | Fraction, or zero for nonpositive capacity. |
| `Logging::setup(writer, options, file_config)` | Owned logger; invalid levels return an I/O input error. |
| `Logging::with_default(operation)` | Runs synchronous work with this dispatch installed. |
| `Logging::dispatch()` | Native dispatch for explicit thread/task propagation. |
| `Logging::finish()` | Closes disk output and returns a recorded write or flush failure. |
| `logging::run_command(writer, service, mode, operation)` | Reports a returned error once, closes logs, and returns status zero or one. The closure can replace its active logger after parsing. |
| `logging::http_error(detail)` | Error event with action `http_server_error` and preserved diagnostic detail. |

Generate the complete local type reference with `cargo doc --no-deps --open`.

# License

[MIT License](LICENSE) (c) 2026 Alex Gorbatchev.
