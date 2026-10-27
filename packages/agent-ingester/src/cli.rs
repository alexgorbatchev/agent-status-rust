//! Native command parsing and service configuration.
use crate::{
    classifier,
    config::{Paths, duration, nonempty},
    summarizer::{self, DisableSource},
};
use clap::{Arg, ArgAction, Command, parser::ValueSource};
use cli_command_kit::{HelpOptions, Mode};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

pub const SKILL_POINTER: &str = "Install the agent-ingester skill using the Skills CLI:\n\n  npx skills add alexgorbatchev/agent-status --skill agent-ingester\n\nSkill:\n\n  https://github.com/alexgorbatchev/agent-status/tree/main/skills/agent-ingester\n";
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Arguments(#[from] clap::Error),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::Error),
}
pub struct Options {
    pub addr: String,
    pub token: String,
    pub database: PathBuf,
    pub notify_socket: PathBuf,
    pub herdr_socket: PathBuf,
    pub classifier: classifier::Config,
    pub summarizer: summarizer::Config,
    pub log_level: String,
    pub log_dir: PathBuf,
    pub log_file: PathBuf,
}
pub enum Action {
    Printed,
    Serve(Box<Options>),
}
pub fn help_options() -> HelpOptions {
    HelpOptions {
        skill: Some(SKILL_POINTER.into()),
        ..Default::default()
    }
}
pub fn command(mode: Mode) -> Command {
    let mut root = Command::new("agent-ingester")
        .about("Classify, persist, and publish acknowledged agent telemetry")
        .version(env!("CARGO_PKG_VERSION"))
        .args_conflicts_with_subcommands(true);
    for (name, default, description) in [
        ("addr", ":4501", "HTTP/WebSocket listen address"),
        (
            "token",
            "",
            "Bearer authentication token; defaults to AGENT_BEARER_TOKEN or dev-secret-token",
        ),
        (
            "db",
            "",
            "SQLite database path; defaults to XDG data directory",
        ),
        (
            "notify-sock",
            "",
            "Unix notification socket; defaults to XDG runtime directory",
        ),
        (
            "litellm-url",
            "",
            "OpenAI-compatible / LiteLLM proxy base URL",
        ),
        ("litellm-key", "", "OpenAI-compatible / LiteLLM API key"),
        (
            "summarizer-model",
            "",
            "Model override for all configured summarizers",
        ),
        ("log-dir", "", "Rotating log directory"),
        ("log-file", "", "Rotating log file path"),
    ] {
        root = root.arg(
            Arg::new(name)
                .long(name)
                .default_value(default)
                .hide_default_value(true)
                .help(description),
        );
    }
    root = root
        .arg(
            Arg::new("no-summarize")
                .long("no-summarize")
                .action(ArgAction::Set)
                .num_args(0..=1)
                .require_equals(true)
                .default_missing_value("true")
                .default_value("false")
                .value_parser(clap::value_parser!(bool))
                .help("Disable LLM action summarization; =false overrides environment disable"),
        )
        .arg(
            Arg::new("idle-grace-period")
                .long("idle-grace-period")
                .default_value("5s")
                .allow_hyphen_values(true)
                .value_parser(duration)
                .help("Grace period before BUSY transitions to IDLE on turn end"),
        )
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
    parse_with_env(
        args,
        mode,
        output,
        |name| std::env::var(name).ok(),
        Paths::capture(),
    )
}
pub fn parse_with_env(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
    mode: Mode,
    mut output: &mut impl Write,
    lookup: impl Fn(&str) -> Option<String>,
    paths: Paths,
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
            clap_complete::generate(shell, &mut root, "agent-ingester", &mut bytes);
            output.write_all(&bytes)?;
        } else {
            write!(
                output,
                "{}",
                root.find_subcommand_mut("completion")
                    .expect("completion exists")
                    .render_help()
            )?;
        }
        return Ok(Action::Printed);
    }
    let flag = |name: &str| matches.value_source(name) == Some(ValueSource::CommandLine);
    let value = |name: &str| {
        matches
            .get_one::<String>(name)
            .expect("string argument")
            .clone()
    };
    let setting = |name: &str, environments: &[&str], fallback: &str| {
        if flag(name) {
            value(name)
        } else {
            environments
                .iter()
                .find_map(|name| nonempty(&lookup, name))
                .unwrap_or_else(|| fallback.into())
        }
    };
    let disabled = if flag("no-summarize") {
        matches
            .get_one::<bool>("no-summarize")
            .copied()
            .unwrap_or_default()
            .then_some(DisableSource::Flag)
    } else {
        (lookup("AGENT_NO_SUMMARIZE").as_deref() == Some("1")).then_some(DisableSource::Environment)
    };
    let mut idle_grace = *matches
        .get_one::<Duration>("idle-grace-period")
        .expect("duration argument");
    if !flag("idle-grace-period")
        && let Some(value) = nonempty(&lookup, "AGENT_IDLE_GRACE_PERIOD")
    {
        match duration(&value) {
            Ok(duration) => idle_grace = duration,
            Err(error) => tracing::error!(raw=%value,%error,"invalid_idle_grace_period"),
        }
    }
    let summarizer = summarizer::Config::from_lookup(disabled, |environment| {
        let override_flag = match environment {
            "LITELLM_BASE_URL" | "OPENAI_BASE_URL" => Some("litellm-url"),
            "LITELLM_API_KEY" | "LITELLM_KEY" | "OPENAI_API_KEY" => Some("litellm-key"),
            "AGENT_SUMMARIZER_MODEL" => Some("summarizer-model"),
            _ => None,
        };
        if let Some(name) = override_flag
            && flag(name)
        {
            Some(value(name))
        } else {
            lookup(environment)
        }
    });
    Ok(Action::Serve(Box::new(Options {
        addr: setting("addr", &["AGENT_INGESTER_ADDR"], ":4501"),
        token: setting("token", &["AGENT_BEARER_TOKEN"], "dev-secret-token"),
        database: if flag("db") {
            value("db").into()
        } else {
            paths.database(&lookup)
        },
        notify_socket: if flag("notify-sock") {
            value("notify-sock").into()
        } else {
            paths.notify_socket(&lookup)
        },
        herdr_socket: paths.herdr_socket(&lookup),
        classifier: classifier::Config {
            idle_grace,
            ..Default::default()
        },
        summarizer,
        log_level: value("log-level"),
        log_dir: setting("log-dir", &["AGENT_LOG_DIR"], "").into(),
        log_file: setting("log-file", &["AGENT_LOG_FILE"], "").into(),
    })))
}
pub fn logging_options(
    options: &Options,
    mode: Mode,
) -> (
    agent_common::logging::Options,
    agent_common::logging::FileConfig,
) {
    let mut file = agent_common::logging::FileConfig::from_env();
    if !options.log_dir.as_os_str().is_empty() {
        file.dir = options.log_dir.clone();
    }
    if !options.log_file.as_os_str().is_empty() {
        file.filename = Some(options.log_file.clone());
    }
    (
        agent_common::logging::Options {
            service: "ingester".into(),
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
