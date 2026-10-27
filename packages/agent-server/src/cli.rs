//! Clap argument parsing and explicit environment defaults; no service work during metadata commands.
use crate::api::ApiConfig;
use clap::{Arg, ArgAction, Command, parser::ValueSource};
use cli_command_kit::{HelpOptions, Mode};
use std::{
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
};

pub const SKILL_POINTER: &str = "Install the agent-server skill using the Skills CLI:\n\n  npx skills add alexgorbatchev/agent-status --skill agent-server\n\nSkill:\n\n  https://github.com/alexgorbatchev/agent-status/tree/main/skills/agent-server\n";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Arguments(#[from] clap::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("environment variable {0} must contain valid UTF-8")]
    InvalidEnvironment(&'static str),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::Error),
}

pub struct Options {
    pub addr: String,
    pub database: PathBuf,
    pub socket: PathBuf,
    pub api: ApiConfig,
    pub log_level: String,
    pub log_dir: PathBuf,
    pub log_file: PathBuf,
}
pub enum Action {
    Printed,
    Serve(Box<Options>),
}
pub struct Paths {
    pub home: Option<PathBuf>,
}
impl Paths {
    pub fn capture() -> Self {
        Self {
            home: dirs::home_dir(),
        }
    }
}

pub fn help_options() -> HelpOptions {
    HelpOptions {
        skill: Some(SKILL_POINTER.into()),
        ..Default::default()
    }
}

fn origins(value: &str) -> Result<Vec<String>, reliakit_csv::CsvError> {
    // Flags hold one record. Reject trailing records instead of silently ignoring them.
    // The OS already bounds argv storage. These ceilings follow the actual value,
    // retaining strict grammar without importing document-size limits into a flag.
    let limits = reliakit_csv::CsvLimits::conservative()
        .with_max_input_bytes(value.len())
        .with_max_field_bytes(value.len())
        .with_max_fields_per_record(value.len().saturating_add(1))
        .with_max_records(1);
    reliakit_csv::read_str_with_limits(value, &limits)
        .map(|records| records.into_iter().next().unwrap_or_default())
}

pub fn command(mode: Mode) -> Command {
    let mut root = Command::new("agent-server")
        .about("Query agent activity and stream live updates")
        .version(env!("CARGO_PKG_VERSION"))
        .args_conflicts_with_subcommands(true);
    for (name, description) in [
        ("addr", "HTTP listen address; default :4502"),
        ("token", "Bearer token; defaults to AGENT_STATUS_TOKEN"),
        (
            "ingester-url",
            "Mutation service URL; defaults to AGENT_INGESTER_HTTP_URL or http://127.0.0.1:4501",
        ),
    ] {
        let mut argument = Arg::new(name).long(name).help(description);
        if name == "addr" {
            argument = argument.short('a').default_value(":4502");
        }
        root = root.arg(argument);
    }
    for (name, description) in [
        (
            "db",
            "Read-only SQLite database; defaults to XDG data directory",
        ),
        (
            "socket",
            "Unix notification socket; defaults to XDG runtime directory",
        ),
        (
            "log-dir",
            "Rotating log directory; defaults to AGENT_LOG_DIR",
        ),
        ("log-file", "Rotating log file; defaults to AGENT_LOG_FILE"),
    ] {
        root = root.arg(
            Arg::new(name)
                .long(name)
                .value_parser(clap::value_parser!(PathBuf))
                .help(description),
        );
    }
    root = root
        .arg(
            Arg::new("cors-origin")
                .long("cors-origin")
                .action(ArgAction::Append)
                .value_parser(origins)
                .help(
                    "Allowed CORS origins; repeat or use a CSV list; default http://localhost:3000",
                ),
        )
        .arg(
            Arg::new("log-level")
                .long("log-level")
                .default_value("info")
                .value_parser(|value: &str| {
                    agent_common::logging::parse_level(value)
                        .map(|_| value.to_owned())
                        .map_err(|error| error.to_string())
                })
                .help("Minimum log level: debug, info, warn, error"),
        )
        .subcommand(
            Command::new("completion")
                .about("Generate shell completion scripts")
                .subcommands(["bash", "zsh", "fish", "powershell"].map(|shell| {
                    Command::new(shell).about(format!("Generate {shell} completion"))
                })),
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
        |name| std::env::var_os(name),
        Paths::capture(),
    )
}

pub fn parse_with_env(
    args: impl IntoIterator<Item = impl Into<OsString> + Clone>,
    mode: Mode,
    mut output: &mut impl Write,
    lookup: impl Fn(&str) -> Option<OsString>,
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
        Err(mut error) => {
            error.remove(clap::error::ContextKind::Usage);
            return Err(error.into());
        }
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
            clap_complete::generate(shell, &mut root, "agent-server", &mut bytes);
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
    let string = |name: &str, environment: &'static str, fallback: &str| -> Result<String, Error> {
        if matches.value_source(name) == Some(ValueSource::CommandLine) {
            return Ok(matches
                .get_one::<String>(name)
                .expect("string argument")
                .clone());
        }
        lookup(environment)
            .filter(|value| !value.is_empty())
            .map_or_else(
                || Ok(fallback.into()),
                |value| {
                    value
                        .into_string()
                        .map_err(|_| Error::InvalidEnvironment(environment))
                },
            )
    };
    let environment_path = |name| {
        lookup(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let path = |name, environment| {
        matches
            .get_one::<PathBuf>(name)
            .cloned()
            .or_else(|| environment_path(environment))
            .unwrap_or_default()
    };
    let database = matches
        .get_one::<PathBuf>("db")
        .cloned()
        .unwrap_or_else(|| {
            environment_path("XDG_DATA_HOME")
                .unwrap_or_else(|| paths.home.clone().unwrap_or_default().join(".local/share"))
                .join("agent-status/agent-status.db")
        });
    let socket = matches
        .get_one::<PathBuf>("socket")
        .cloned()
        .unwrap_or_else(|| {
            environment_path("XDG_RUNTIME_DIR")
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("agent-status/notify.sock")
        });
    let cors_origins = matches.get_many::<Vec<String>>("cors-origin").map_or_else(
        || ApiConfig::default().cors_origins,
        |values| values.flatten().cloned().collect(),
    );
    Ok(Action::Serve(Box::new(Options {
        addr: matches
            .get_one::<String>("addr")
            .expect("address argument")
            .clone(),
        database,
        socket,
        api: ApiConfig {
            ingester_url: string(
                "ingester-url",
                "AGENT_INGESTER_HTTP_URL",
                "http://127.0.0.1:4501",
            )?,
            auth_token: string("token", "AGENT_STATUS_TOKEN", "")?,
            cors_origins,
            ..Default::default()
        },
        log_level: matches
            .get_one::<String>("log-level")
            .expect("log level argument")
            .clone(),
        log_dir: path("log-dir", "AGENT_LOG_DIR"),
        log_file: path("log-file", "AGENT_LOG_FILE"),
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
            service: "server".into(),
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
