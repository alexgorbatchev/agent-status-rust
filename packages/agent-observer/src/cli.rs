//! Offline command parsing, followed by an explicitly cancellable daemon lifecycle.
use crate::{
    config::{Identity, Paths, resolve_label},
    runtime, shipper,
};
use clap::{Arg, Command, parser::ValueSource};
use cli_command_kit::{HelpOptions, Mode};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const SKILL_POINTER: &str = "Install the agent-observer skill using the Skills CLI:\n\n  npx skills add alexgorbatchev/agent-status --skill agent-observer\n\nSkill:\n\n  https://github.com/alexgorbatchev/agent-status/tree/main/skills/agent-observer\n";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Arguments(#[from] clap::Error),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Runtime(#[from] runtime::Error),
}

pub struct Options {
    pub ingester_url: Url,
    pub token: String,
    pub environment_label: String,
    pub watcher: agent_watcher::Config,
    pub log_level: String,
    pub log_file: PathBuf,
    pub log_dir: PathBuf,
}

pub enum Action {
    Printed,
    Observe(Box<Options>),
}

pub fn help_options() -> HelpOptions {
    let mut options = HelpOptions {
        skill: Some(SKILL_POINTER.into()),
        ..Default::default()
    };
    options
        .catalog
        .entry("agent-observer".into())
        .or_default()
        .metadata
        .insert("type:ingester-url".into(), "string".into());
    options
}

fn duration(value: &str) -> Result<Duration, String> {
    if matches!(value, "0" | "+0" | "-0") {
        return Ok(Duration::ZERO);
    }
    let nanos = value
        .parse::<go_duration::GoDuration>()
        .map_err(|_| "expected a duration such as 2s, 250ms, or 24h".to_string())?
        .nanoseconds();
    u64::try_from(nanos)
        .map(Duration::from_nanos)
        .map_err(|_| "duration must be nonnegative".into())
}

fn websocket_url(value: &str) -> Result<Url, String> {
    Url::parse(value)
        .ok()
        .filter(|url| matches!(url.scheme(), "ws" | "wss") && url.host_str().is_some())
        .ok_or_else(|| "ingester URL must be an absolute ws:// or wss:// URL".into())
}

pub fn command(mode: Mode) -> Command {
    let mut root = Command::new("agent-observer")
        .about("Monitor local agent sessions and deliver acknowledged telemetry")
        .version(env!("CARGO_PKG_VERSION"))
        .args_conflicts_with_subcommands(true)
        .arg(
            Arg::new("ingester-url")
                .long("ingester-url")
                .default_value("ws://127.0.0.1:4501/ws/ingest")
                .value_parser(websocket_url)
                .help("Ingester WebSocket URL"),
        );
    for (name, help) in [
        ("token", "Bearer token for ingester authentication"),
        ("env", "Environment label override"),
        ("cursor-path", "Persistent transcript cursor file"),
        (
            "session-state-path",
            "Persistent discovered session state file",
        ),
        (
            "claude-sessions-dir",
            "Claude Code session descriptor directory",
        ),
        ("claude-projects-dir", "Claude Code transcript directory"),
        ("pi-sessions-dir", "Pi session directory"),
        ("codex-sessions-dir", "Codex session directory"),
        ("opencode-db-path", "OpenCode SQLite database"),
        ("log-file", "Rotating log file path"),
        ("log-dir", "Rotating log directory"),
    ] {
        root = root.arg(Arg::new(name).long(name).default_value("").help(help));
    }
    for (name, default, help) in [
        ("scan-interval", "2s", "Interval between discovery scans"),
        ("cursor-retention", "24h", "Inactive cursor retention"),
        (
            "cursor-prune-interval",
            "1h",
            "Interval between cursor pruning sweeps",
        ),
        (
            "cursor-flush-interval",
            "2s",
            "Interval between persistent cursor flushes",
        ),
    ] {
        root = root.arg(
            Arg::new(name)
                .long(name)
                .default_value(default)
                .allow_hyphen_values(true)
                .value_parser(duration)
                .help(help),
        );
    }
    root = root
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .default_value("info")
                .value_parser(|value: &str| {
                    agent_common::logging::parse_level(value)
                        .map(|_| value.to_string())
                        .map_err(|error| error.to_string())
                })
                .help("Minimum log level"),
        )
        .subcommand(
            Command::new("completion")
                .about("Generate shell completion scripts")
                .subcommands(
                    ["bash", "zsh", "fish", "powershell"].map(|name| {
                        Command::new(name).about(format!("Generate {name} completion"))
                    }),
                ),
        );
    cli_command_kit::setup(root, mode, &help_options()).expect("static command tree is valid")
}

pub fn parse(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
    mode: Mode,
    output: &mut impl Write,
) -> Result<Action, Error> {
    parse_with_env(args, mode, output, |key| std::env::var(key).ok())
}

pub fn parse_with_env(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
    mode: Mode,
    mut output: &mut impl Write,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Action, Error> {
    let mut root = command(mode);
    let matches = match root.clone().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayVersion => {
            writeln!(output, "{}", env!("CARGO_PKG_VERSION"))?;
            return Ok(Action::Printed);
        }
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            write!(output, "{error}")?;
            return Ok(Action::Printed);
        }
        Err(error) => return Err(error.into()),
    };
    if cli_command_kit::write_skill(&matches, &help_options(), &mut output)? {
        return Ok(Action::Printed);
    }
    if let Some(("completion", matches)) = matches.subcommand() {
        if let Some((name, _)) = matches.subcommand() {
            let shell = match name {
                "bash" => clap_complete::Shell::Bash,
                "zsh" => clap_complete::Shell::Zsh,
                "fish" => clap_complete::Shell::Fish,
                _ => clap_complete::Shell::PowerShell,
            };
            let mut bytes = Vec::new();
            clap_complete::generate(shell, &mut root, "agent-observer", &mut bytes);
            output.write_all(&bytes)?;
        } else {
            write!(
                output,
                "{}",
                root.find_subcommand_mut("completion")
                    .expect("exists")
                    .render_help()
            )?;
        }
        return Ok(Action::Printed);
    }
    let value = |name: &str| {
        matches
            .get_one::<String>(name)
            .expect("string default")
            .clone()
    };
    let environment_default = |name: &str, env: &str| {
        if matches.value_source(name) == Some(ValueSource::CommandLine) {
            value(name)
        } else {
            lookup(env)
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| value(name))
        }
    };
    let interval = |name: &str, env: &str| {
        let parsed = *matches.get_one::<Duration>(name).expect("duration default");
        if matches.value_source(name) == Some(ValueSource::CommandLine) {
            parsed
        } else {
            lookup(env)
                .and_then(|v| duration(&v).ok())
                .unwrap_or(parsed)
        }
    };
    let ingester_url = if matches.value_source("ingester-url") == Some(ValueSource::CommandLine) {
        matches
            .get_one::<Url>("ingester-url")
            .expect("URL parser")
            .clone()
    } else if let Some(url) = lookup("AGENT_INGESTER_WS_URL").filter(|v| !v.is_empty()) {
        websocket_url(&url)
            .map_err(|message| clap::Error::raw(clap::error::ErrorKind::ValueValidation, message))?
    } else {
        matches
            .get_one::<Url>("ingester-url")
            .expect("URL default")
            .clone()
    };
    let claude = environment_default("claude-sessions-dir", "CLAUDE_SESSIONS_DIR");
    let watcher = agent_watcher::Config {
        scan_interval: *matches
            .get_one::<Duration>("scan-interval")
            .expect("duration"),
        cursor_path: value("cursor-path").into(),
        session_state_path: environment_default("session-state-path", "AGENT_SESSION_STATE_PATH")
            .into(),
        claude_sessions_dirs: if claude.is_empty() {
            vec![]
        } else {
            vec![claude.into()]
        },
        claude_projects_dir: value("claude-projects-dir").into(),
        pi_sessions_dir: value("pi-sessions-dir").into(),
        codex_sessions_dir: value("codex-sessions-dir").into(),
        opencode_db_path: value("opencode-db-path").into(),
        cursor_retention: interval("cursor-retention", "AGENT_CURSOR_RETENTION"),
        cursor_prune_interval: interval("cursor-prune-interval", "AGENT_CURSOR_PRUNE_INTERVAL"),
        cursor_flush_interval: interval("cursor-flush-interval", "AGENT_CURSOR_FLUSH_INTERVAL"),
        ..Default::default()
    };
    Ok(Action::Observe(Box::new(Options {
        ingester_url,
        token: environment_default("token", "AGENT_STATUS_TOKEN"),
        environment_label: value("env"),
        watcher,
        log_level: value("log-level"),
        log_file: value("log-file").into(),
        log_dir: value("log-dir").into(),
    })))
}

/// Environment identity is captured only after offline metadata/argument handling.
pub async fn observe(cancel: &CancellationToken, options: Options) -> Result<(), Error> {
    let identity = Identity::capture();
    let paths = Paths::capture();
    let label = resolve_label(
        &options.environment_label,
        &std::env::var("AI_ENV").unwrap_or_default(),
        paths.telemetry_config().as_deref(),
        &identity.hostname,
    );
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let environment = identity.environment(label, i64::try_from(now).unwrap_or(i64::MAX));
    tracing::info!(env=%environment.label, env_id=%environment.id, "observer_starting");
    let mut shipper = shipper::Config::new(options.ingester_url, environment);
    shipper.bearer_token = options.token;
    let result = runtime::run(cancel, options.watcher, shipper).await;
    tracing::info!("observer_shutting_down");
    result.map_err(Into::into)
}

pub fn logging_options(
    options: &Options,
    mode: Mode,
) -> (
    agent_common::logging::Options,
    agent_common::logging::FileConfig,
) {
    let mut file = agent_common::logging::FileConfig::from_env();
    if !options.log_file.as_os_str().is_empty() {
        file.filename = Some(options.log_file.clone());
    }
    if !options.log_dir.as_os_str().is_empty() {
        file.dir = options.log_dir.clone();
    }
    (
        agent_common::logging::Options {
            service: "observer".into(),
            level: options.log_level.clone(),
            mode: if mode == Mode::Agent {
                agent_common::logging::Mode::Agent
            } else {
                agent_common::logging::Mode::Human
            },
        },
        file,
    )
}

/// A console record is UTF-8 because the shared tracing layer formats it atomically.
/// Disk records keep their full paths; only displayed records fold the home prefix.
pub struct Console<W>(pub W);
impl<W: Write> Write for Console<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let text = std::str::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.0
            .write_all(cli_command_kit::display_message(text).as_bytes())?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
