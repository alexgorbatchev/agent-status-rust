use clap::{Arg, ArgAction, Command};
use cli_command_kit::{HelpOptions, Mode};
pub const SKILL_POINTER: &str = "Install the agent-debug skill using the Skills CLI:\n\n  npx skills add alexgorbatchev/agent-status --skill agent-debug\n\nSkill:\n\n  https://github.com/alexgorbatchev/agent-status/tree/main/skills/agent-debug\n";
pub fn help_options() -> HelpOptions {
    HelpOptions {
        skill: Some(SKILL_POINTER.into()),
        ..Default::default()
    }
}
fn string(name: &'static str, default: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).default_value(default).help(help)
}
pub(super) fn boolean(name: &'static str, default: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .action(ArgAction::Set)
        .num_args(0..=1)
        .require_equals(true)
        .default_missing_value("true")
        .default_value(default)
        .value_parser(clap::value_parser!(bool))
        .help(help)
}
fn json(command: Command) -> Command {
    command.arg(boolean("json", "false", "Write structured JSON"))
}
fn harness(command: Command) -> Command {
    command.arg(
        string("harness", "", "Override transcript harness").value_parser([
            "",
            "claude-code",
            "pi",
            "codex",
            "opencode",
        ]),
    )
}
fn session_options(command: Command) -> Command {
    harness(json(command))
        .arg(string(
            "session-id",
            "",
            "Select or override session identity",
        ))
        .arg(string(
            "cwd",
            "",
            "Override transcript working directory for local Git resolution",
        ))
        .arg(string("env", "debug", "Replay environment label"))
}
fn breakpoint(command: Command) -> Command {
    command
        .arg(
            Arg::new("until-line")
                .long("until-line")
                .default_value("0")
                .value_parser(clap::value_parser!(usize))
                .help("Stop after source line N; zero processes all lines"),
        )
        .arg(string(
            "until-event",
            "",
            "Stop after the first matching telemetry event",
        ))
}
fn strict(command: Command) -> Command {
    command.arg(
        boolean(
            "strict-lifecycle",
            "false",
            "Require native lifecycle boundaries",
        )
        .visible_alias("no-synthetic-lifecycle"),
    )
}
fn target(command: Command) -> Command {
    command.arg(
        Arg::new("target")
            .value_name("file-or-session-id")
            .required(true),
    )
}
fn fleet_inspect() -> Command {
    harness(json(
        Command::new("inspect").about("Group agent sessions by project and worktree"),
    ))
    .arg(
        Arg::new("targets")
            .value_name("file-or-database-or-session-id")
            .num_args(0..),
    )
    .arg(string(
        "db",
        "auto",
        "Monitoring database when no targets are supplied",
    ))
    .arg(string("cwd", "", "Override transcript working directory"))
    .arg(string("env", "debug", "Replay environment label"))
}
fn event_list() -> Command {
    json(Command::new("list").about("List recorded telemetry events"))
        .arg(Arg::new("session-id").required(true))
        .arg(string("db", "auto", "Monitoring database path"))
        .arg(
            Arg::new("limit")
                .long("limit")
                .default_value("20")
                .value_parser(clap::value_parser!(i64).range(0..))
                .help("Maximum events; zero is unlimited"),
        )
        .arg(string("type", "", "Comma-separated telemetry event types"))
        .arg(string("subagent-id", "", "Filter by subagent identity"))
        .arg(boolean(
            "include-subagents",
            "true",
            "Include subagent events",
        ))
        .arg(boolean("desc", "true", "Order newest events first"))
        .arg(boolean(
            "asc",
            "false",
            "Order oldest first; overrides desc",
        ))
}
fn audit(name: &'static str, about: &'static str) -> Command {
    json(Command::new(name).about(about))
        .arg(
            string("harness", "", "Filter harness")
                .value_parser(["", "claude", "pi", "codex", "opencode"]),
        )
        .arg(string(
            "cwd",
            "",
            "Filter working directory and descendants",
        ))
}
pub fn command(mode: Mode) -> Command {
    let root = Command::new("agent-debug").about("Replay, validate, and inspect Agent Status sessions")
        .version(env!("CARGO_PKG_VERSION")).arg(boolean("debug", "false", "Write diagnostic logs to stderr").global(true))
        .subcommand(Command::new("session").about("Inspect agent session transcripts")
            .subcommand(Command::new("list").about("List sessions in an OpenCode database").arg(Arg::new("target").value_name("file-or-database").required(true)))
            .subcommand(strict(breakpoint(session_options(target(Command::new("replay").about("Replay through the offline ingestion pipeline")))))
                .arg(boolean("diff", "false", "Display state changes during replay"))
                .arg(boolean("stop-on-error", "false", "Stop on the first parse or ingestion error"))
                .arg(boolean("summarize", "false", "Enable configured live LLM providers"))
                .arg(string("export-db", "", "Write replayed telemetry to this SQLite database"))
                .arg(string("db", "", "Compare with this monitoring database; auto discovers it")))
            .subcommand(strict(session_options(target(Command::new("validate").about("Validate framing, lifecycle, usage, and read queries"))))
                .arg(string("db", "", "Compare with this monitoring database; auto discovers it")))
            .subcommand(breakpoint(session_options(target(Command::new("view").about("Show an agent session card"))))))
        .subcommand(Command::new("fleet").about("Inspect groups of agent sessions").subcommand(fleet_inspect()))
        .subcommand(Command::new("event").about("Inspect recorded telemetry events")
            .subcommand(event_list())
            .subcommand(json(Command::new("inspect").about("Inspect a recorded telemetry event"))
                .arg(Arg::new("id").required(true).value_parser(clap::value_parser!(i64).range(1..)).help("Positive telemetry event ID"))
                .arg(string("db", "auto", "Monitoring database path"))))
        .subcommand(Command::new("db").about("Inspect persisted monitoring records").subcommand(json(Command::new("inspect").about("Inspect an agent and its runs"))
            .arg(Arg::new("session-id").required(true)).arg(string("db", "auto", "Monitoring database path"))))
        .subcommand(Command::new("scanner").about("Audit live process and transcript pairing")
            .subcommand(audit("audit", "Audit process discovery and orphan sessions"))
            .subcommand(audit("list", "List process and session pairing; alias for audit")))
        .subcommand(Command::new("process").about("Inspect harness processes")
            .subcommand(audit("list", "List process and session pairing; alias for audit")))
        .subcommand(Command::new("agent").about("Discover active agents and inspect usage")
            .subcommand(Command::new("list").about("Write headerless PID, harness, session, hostname, directory, and title TSV")
                .arg(boolean("all", "false", "Read all monitored machines from agent-server").conflicts_with("db"))
                .arg(Arg::new("db").long("db").help("Read local monitoring database instead of scanning"))
                .arg(Arg::new("server-url").long("server-url").requires("all").help("Server HTTP base URL; defaults to AGENT_SERVER_HTTP_URL or http://127.0.0.1:4502")))
            .subcommand(json(Command::new("usage").about("Inspect server-reported session usage"))
                .arg(Arg::new("agent-id").required(true))
                .arg(Arg::new("server-url").long("server-url").help("Server HTTP base URL; defaults to AGENT_SERVER_HTTP_URL or http://127.0.0.1:4502"))))
        .subcommand(Command::new("completion").about("Generate shell completion scripts").subcommands(["bash", "zsh", "fish", "powershell"].map(|name| Command::new(name).about(format!("Generate {name} completion")))));
    cli_command_kit::setup(root, mode, &help_options()).expect("static command tree is valid")
}
